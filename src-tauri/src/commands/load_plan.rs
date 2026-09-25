//! `qm preflight`: the request Modkit writes and the `load-plan.json` qm writes back.
//!
//! Quartermaster owns both formats. This module is their serde mirror and nothing more: every
//! shape here follows what qm writes and reads, and where the two ever disagree qm wins and
//! this file is wrong.
//!
//! # Refuse, never guess
//!
//! * Every key is always present in a plan. Keys that don't apply are `null`, not missing. So
//!   every struct is `deny_unknown_fields` and no field has a serde default. `Option` fields go
//!   through [`nullable`], because serde would otherwise read a *missing* `Option` as `None`.
//! * Every closed set is an enum, so an unknown value fails to parse.
//! * `format` is this file's own version (`1`). The manifest format is a different number.
//!
//! # What Modkit does with it
//!
//! Modkit runs `qm preflight` before every Shipment build and refuses to build when `ok` is
//! false. The dependency resolver and the removal cascade read the plan's `requirements` rather
//! than parsing any manifest's `load` table themselves: Modkit never parses `load` itself, and
//! `required_by` is derived from the latest plan each time, so it can't drift.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Deserializer, Serialize};

use super::proc::NoWindow;
use super::shipment::ShipmentRef;

/// The `format` of `load-request.json` and `load-plan.json`. It is not the
/// manifest format.
pub const PLAN_FORMAT: u32 = 1;

/// The request file's name inside a work dir. `qm link --request` is given the same file
/// preflight was given.
pub const REQUEST_FILE: &str = "load-request.json";

/// The plan file's name inside `--out`.
pub const PLAN_FILE: &str = "load-plan.json";

/// A key that is always present but may be `null`. serde reads a *missing* `Option` field as
/// `None` unless a `deserialize_with` is set; setting one makes a missing key an error.
pub(crate) fn nullable<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d)
}

// ---------------------------------------------------------------------------------------
// load-request.json
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct LoadRequest {
    pub format: u32,
    pub items: Vec<RequestItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestItem {
    pub id: String,
    pub path: String,
}

impl LoadRequest {
    /// One item per row, in the order given. That order is the Kahn tie-break.
    pub fn for_rows(rows: &[ShipmentRef]) -> Self {
        LoadRequest {
            format: PLAN_FORMAT,
            items: rows
                .iter()
                .map(|r| RequestItem { id: r.id.clone(), path: r.path.clone() })
                .collect(),
        }
    }
}

// ---------------------------------------------------------------------------------------
// load-plan.json
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Producer {
    Preflight,
    Link,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadPlan {
    pub format: u32,
    pub producer: Producer,
    pub quartermaster: String,
    pub ok: bool,
    #[serde(deserialize_with = "nullable")]
    pub order: Option<Vec<String>>,
    pub items: Vec<PlanItem>,
    pub edges: Vec<Edge>,
    pub requirements: Vec<Requirement>,
    pub capabilities: Vec<CapabilityRow>,
    pub conflicts: Vec<ConflictRow>,
    pub supersedes: Vec<SupersedeRow>,
    /// Every block `qm link` re-emits: the script blocks plus the merged string-table blocks
    /// (renamed from `script_block_paths`, which no longer fit once string tables joined the
    /// list).
    pub link_block_paths: Vec<String>,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanItem {
    pub id: String,
    pub requested: usize,
    #[serde(deserialize_with = "nullable")]
    pub resolved: Option<usize>,
    #[serde(deserialize_with = "nullable")]
    pub held_back_by: Option<usize>,
    pub name: String,
    pub version: String,
    pub manifest_format: u32,
    #[serde(deserialize_with = "nullable")]
    pub quartermaster_range: Option<String>,
    pub provides: Vec<String>,
    pub plugins: Vec<FileEntry>,
    pub runtime_dlls: Vec<FileEntry>,
    pub placed_files: Vec<PlacedFileEntry>,
}

/// A `plugins[]` or `runtime_dlls[]` entry. The two share their keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub contribution: usize,
    pub file_name: String,
    pub source: String,
    pub relative: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Destination {
    GameFolder,
    DataWad,
}

/// qm's `PlaceIn`, snake_case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceIn {
    GameRoot,
    Scripts,
    Plugins,
    Update,
    OnBoot,
    OnLoad,
    OnKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacedFileEntry {
    pub contribution: usize,
    pub file_name: String,
    #[serde(deserialize_with = "nullable")]
    pub source: Option<String>,
    pub destination: Destination,
    #[serde(deserialize_with = "nullable")]
    pub dest: Option<PlaceIn>,
    pub relative: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeSource {
    Requires,
    Capability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub first: String,
    pub then: String,
    pub source: EdgeSource,
    pub requirement: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementKind {
    Shipment,
    Capability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementStatus {
    Satisfied,
    Missing,
    VersionUnsatisfied,
    Ambiguous,
}

/// One row per declared `load.requires` entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub consumer: String,
    pub index: usize,
    pub kind: RequirementKind,
    pub target: String,
    #[serde(deserialize_with = "nullable")]
    pub range: Option<String>,
    pub providers: Vec<String>,
    #[serde(deserialize_with = "nullable")]
    pub resolved_version: Option<String>,
    pub status: RequirementStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRow {
    pub capability: String,
    pub consumers: Vec<String>,
    pub providers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclaredStatus {
    Conflict,
    OutsideRange,
    NotInstalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimClass {
    Exclusive,
    KeyedSet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claimant {
    pub item: String,
    pub contribution: usize,
    pub kind: String,
}

/// A `conflicts[]` row. Its two shapes are told apart by `source`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConflictRow {
    Declared {
        declarer: String,
        index: usize,
        named: String,
        #[serde(deserialize_with = "nullable")]
        range: Option<String>,
        installed: Vec<String>,
        status: DeclaredStatus,
    },
    Claims {
        claim: String,
        class: ClaimClass,
        claimants: Vec<Claimant>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupersedeRow {
    pub declared_by: String,
    pub index: usize,
    pub dest: PlaceIn,
    pub file: String,
    pub relative: String,
    pub present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefSection {
    Items,
    Edges,
    Requirements,
    Conflicts,
    Supersedes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingRef {
    pub section: RefSection,
    pub index: usize,
}

/// The one findings element.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub items: Vec<String>,
    pub refs: Vec<FindingRef>,
    #[serde(deserialize_with = "nullable")]
    pub fix: Option<String>,
}

/// The cycle code: `order` is `null` exactly when a finding carries it.
const CYCLE_CODE: &str = "M0174";

impl LoadPlan {
    /// Parse a plan and check it against the request it answers.
    pub fn parse(text: &str, request: &LoadRequest, producer: Producer) -> Result<Self, String> {
        let plan: LoadPlan = serde_json::from_str(text)
            .map_err(|e| format!("qm wrote a load-plan.json Modkit cannot read: {e}"))?;
        plan.validate(request, producer)?;
        Ok(plan)
    }

    /// The invariants of the plan format that Modkit relies on. Any breach is a hard error: a plan
    /// that doesn't answer the request it was given can't be mapped back to rows.
    fn validate(&self, request: &LoadRequest, producer: Producer) -> Result<(), String> {
        if self.format != PLAN_FORMAT {
            return Err(format!(
                "load-plan.json has format {}; Modkit reads only format {PLAN_FORMAT}",
                self.format
            ));
        }
        if self.producer != producer {
            return Err(format!(
                "load-plan.json was written by {:?}, expected {producer:?}",
                self.producer
            ));
        }
        if self.items.len() != request.items.len() {
            return Err(format!(
                "load-plan.json has {} items for a request of {}",
                self.items.len(),
                request.items.len()
            ));
        }
        for (i, (item, want)) in self.items.iter().zip(&request.items).enumerate() {
            if item.id != want.id || item.requested != i {
                return Err(format!(
                    "load-plan.json item {i} is {} (requested {}), but the request's item {i} is {}",
                    item.id, item.requested, want.id
                ));
            }
            if item.manifest_format != 2 {
                return Err(format!(
                    "load-plan.json reports manifest format {} for {}; format 2 is the only one",
                    item.manifest_format, item.id
                ));
            }
        }
        let has_cycle = self.findings.iter().any(|f| f.code == CYCLE_CODE);
        match &self.order {
            Some(order) => {
                if has_cycle {
                    return Err(format!(
                        "load-plan.json has an order and an {CYCLE_CODE} cycle finding; the schema allows only one"
                    ));
                }
                map_order(order, request)?;
            }
            None if !has_cycle => {
                return Err(format!(
                    "load-plan.json has no order but no {CYCLE_CODE} cycle finding"
                ))
            }
            None => {}
        }
        let has_error = self.findings.iter().any(|f| f.severity == Severity::Error);
        if self.ok == has_error {
            return Err(format!(
                "load-plan.json says ok = {} but {} an error finding",
                self.ok,
                if has_error { "carries" } else { "carries no" }
            ));
        }
        for r in &self.requirements {
            let allowed = match r.kind {
                RequirementKind::Shipment => true,
                RequirementKind::Capability => matches!(
                    r.status,
                    RequirementStatus::Satisfied | RequirementStatus::Missing
                ),
            };
            if !allowed {
                return Err(format!(
                    "load-plan.json gives capability requirement {} of {} the status {:?}, which only shipment requirements may have",
                    r.target, r.consumer, r.status
                ));
            }
        }
        Ok(())
    }

    /// The plan's findings as text, with each request id replaced by its row's name. Used for
    /// the refusal message, which the player reads.
    pub fn describe_findings(&self, names: &HashMap<String, String>) -> String {
        self.findings
            .iter()
            .map(|f| {
                let sev = match f.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                };
                let rows: Vec<&str> = f
                    .items
                    .iter()
                    .map(|id| names.get(id).map(String::as_str).unwrap_or(id.as_str()))
                    .collect();
                if rows.is_empty() {
                    format!("{} {sev}: {}", f.code, f.message)
                } else {
                    format!("{} {sev}: {} ({})", f.code, f.message, rows.join(", "))
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Refuse when `ok` is false.
    pub fn refuse_unless_ok(&self, rows: &[ShipmentRef]) -> Result<(), String> {
        if self.ok {
            return Ok(());
        }
        let names: HashMap<String, String> =
            rows.iter().map(|r| (r.id.clone(), r.name.clone())).collect();
        Err(format!(
            "qm preflight refused this set of Shipments, so nothing was built:\n{}",
            self.describe_findings(&names)
        ))
    }

    /// Every `shipment`-kind and `capability`-kind requirement, grouped by consumer id.
    pub fn requirements_by_consumer(&self) -> BTreeMap<&str, Vec<&Requirement>> {
        let mut out: BTreeMap<&str, Vec<&Requirement>> = BTreeMap::new();
        for r in &self.requirements {
            out.entry(r.consumer.as_str()).or_default().push(r);
        }
        out
    }
}

/// Map the plan's `order` back to request indices. Every requested id must appear exactly once;
/// a missing, extra or repeated id is a hard error.
pub fn map_order(order: &[String], request: &LoadRequest) -> Result<Vec<usize>, String> {
    let index: HashMap<&str, usize> = request
        .items
        .iter()
        .enumerate()
        .map(|(i, it)| (it.id.as_str(), i))
        .collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::with_capacity(order.len());
    for id in order {
        let Some(&i) = index.get(id.as_str()) else {
            return Err(format!("load-plan.json's order names {id}, which was not requested"));
        };
        if !seen.insert(id.as_str()) {
            return Err(format!("load-plan.json's order names {id} twice"));
        }
        out.push(i);
    }
    if let Some(missing) = request.items.iter().find(|it| !seen.contains(it.id.as_str())) {
        return Err(format!("load-plan.json's order leaves out {}", missing.id));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// Running qm preflight
// ---------------------------------------------------------------------------------------

/// Write `load-request.json` into `work` and run
/// `qm preflight --request <file> --out <work> --game <game>`.
///
/// Exit 0 and 1 both write a plan, which is read and checked here; exit 0 must carry `ok: true`
/// and exit 1 `ok: false`. Exit 2 means qm could not run, and its stderr is the error. Any other
/// exit, or a plan missing after exit 0 or 1, is an error too. The caller decides what `ok:
/// false` means for it: a build refuses (see [`LoadPlan::refuse_unless_ok`]), while the resolver
/// and the removal cascade still read the requirement data.
pub fn run_preflight(
    qm: &Path,
    rows: &[ShipmentRef],
    game: &OsStr,
    work: &Path,
) -> Result<LoadPlan, String> {
    let request = LoadRequest::for_rows(rows);
    let request_path = work.join(REQUEST_FILE);
    let text = serde_json::to_string_pretty(&request)
        .map_err(|e| format!("building load-request.json: {e}"))?;
    std::fs::write(&request_path, text)
        .map_err(|e| format!("writing {}: {e}", request_path.display()))?;

    let output = Command::new(qm)
        .arg("preflight")
        .arg("--request")
        .arg(&request_path)
        .arg("--out")
        .arg(work)
        .arg("--game")
        .arg(game)
        .no_window()
        .output()
        .map_err(|e| format!("running qm preflight: {e}"))?;

    let code = output.status.code();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let expect_ok = match code {
        Some(0) => true,
        Some(1) => false,
        Some(2) => {
            return Err(format!("qm preflight could not run (exit 2):\n{stderr}"));
        }
        other => {
            return Err(format!(
                "qm preflight ended with {} — not one of its documented exits (0, 1, 2):\n{stderr}",
                other.map(|c| format!("exit {c}")).unwrap_or_else(|| "no exit code".into())
            ));
        }
    };

    let plan_path: PathBuf = work.join(PLAN_FILE);
    let text = std::fs::read_to_string(&plan_path).map_err(|e| {
        format!(
            "qm preflight exited {} but {} could not be read: {e}",
            code.unwrap_or(-1),
            plan_path.display()
        )
    })?;
    let plan = LoadPlan::parse(&text, &request, Producer::Preflight)?;
    if plan.ok != expect_ok {
        return Err(format!(
            "qm preflight exited {} but its plan says ok = {}",
            code.unwrap_or(-1),
            plan.ok
        ));
    }
    Ok(plan)
}

/// Read the plan `qm link` wrote into `out` for `request`. A missing plan is a refusal: `qm link`
/// writes one on every successful path.
pub fn read_link_plan(out: &Path, request: &LoadRequest) -> Result<LoadPlan, String> {
    let path = out.join(PLAN_FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "qm link finished but wrote no readable {PLAN_FILE} ({}: {e}), so its order can't be checked against preflight's",
            path.display()
        )
    })?;
    LoadPlan::parse(&text, request, Producer::Link)
}

/// Refuse when link's plan disagrees with preflight's about `order` or `edges`.
/// Findings may differ: link adds its own M0209 warnings.
pub fn assert_same_plan(preflight: &LoadPlan, link: &LoadPlan) -> Result<(), String> {
    if preflight.order != link.order {
        return Err(format!(
            "qm link resolved a different order than qm preflight did ({:?} vs {:?}), so the build was refused",
            link.order, preflight.order
        ));
    }
    if preflight.edges != link.edges {
        return Err(
            "qm link resolved different requirement edges than qm preflight did, so the build was refused"
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn request(ids: &[&str]) -> LoadRequest {
        LoadRequest {
            format: PLAN_FORMAT,
            items: ids
                .iter()
                .map(|id| RequestItem { id: (*id).into(), path: format!("/x/{id}") })
                .collect(),
        }
    }

    /// A three-Shipment chain: ess requires lua-bridge, my-mod requires ess. There is no m2-sdk
    /// row: Ess requires only lua-bridge, and Modkit does not add m2-sdk to a build. The player
    /// listed ess before lua-bridge, so ess is held back.
    pub(crate) const CHAIN_PLAN: &str = r#"{
      "format": 1, "producer": "preflight", "quartermaster": "3.0.0", "ok": true,
      "order": ["shipment:lua-bridge", "shipment:ess", "shipment:my-mod"],
      "items": [
        { "id": "shipment:ess", "requested": 0, "resolved": 1, "held_back_by": 0,
          "name": "ess", "version": "0.7.0", "manifest_format": 2, "quartermaster_range": null,
          "provides": [], "plugins": [], "runtime_dlls": [], "placed_files": [] },
        { "id": "shipment:lua-bridge", "requested": 1, "resolved": 0, "held_back_by": null,
          "name": "lua-bridge", "version": "1.0.0", "manifest_format": 2, "quartermaster_range": null,
          "provides": [],
          "plugins": [ { "contribution": 0, "file_name": "lua_bridge_DEV.asi", "source": "lua_bridge_DEV.asi",
                         "relative": "scripts/lua_bridge_DEV.asi", "sha256": "00" } ],
          "runtime_dlls": [],
          "placed_files": [ { "contribution": 1, "file_name": "lua_bridge_DEV.ini", "source": "lua_bridge_DEV.ini",
                              "destination": "game_folder", "dest": "scripts", "relative": "scripts/lua_bridge_DEV.ini" } ] },
        { "id": "shipment:my-mod", "requested": 2, "resolved": 2, "held_back_by": null,
          "name": "my-mod", "version": "1.0.0", "manifest_format": 2, "quartermaster_range": null,
          "provides": [], "plugins": [], "runtime_dlls": [], "placed_files": [] }
      ],
      "edges": [
        { "first": "shipment:lua-bridge", "then": "shipment:ess",    "source": "requires", "requirement": 0 },
        { "first": "shipment:ess",        "then": "shipment:my-mod", "source": "requires", "requirement": 1 }
      ],
      "requirements": [
        { "consumer": "shipment:ess", "index": 0, "kind": "shipment", "target": "lua-bridge",
          "range": "^1.0.0", "providers": ["shipment:lua-bridge"],
          "resolved_version": "1.0.0", "status": "satisfied" },
        { "consumer": "shipment:my-mod", "index": 0, "kind": "shipment", "target": "ess",
          "range": ">=0.7, <1", "providers": ["shipment:ess"],
          "resolved_version": "0.7.0", "status": "satisfied" }
      ],
      "capabilities": [],
      "conflicts": [
        { "source": "declared", "declarer": "shipment:my-mod", "index": 0, "named": "other",
          "range": null, "installed": [], "status": "not_installed" },
        { "source": "claims", "claim": "file:scripts/x.ini", "class": "exclusive",
          "claimants": [ { "item": "shipment:ess", "contribution": 0, "kind": "place_file" } ] }
      ],
      "supersedes": [
        { "declared_by": "shipment:ess", "index": 0, "dest": "on_load", "file": "1_Ess.lua",
          "relative": "scripts/OnLoad/1_Ess.lua", "present": false }
      ],
      "link_block_paths": ["blocks\\VZ\\scripts_vz_P000_Q3.block", "blocks\\VZ\\resident_P000_Q3.block"],
      "findings": []
    }"#;

    pub(crate) fn chain_request() -> LoadRequest {
        request(&["shipment:ess", "shipment:lua-bridge", "shipment:my-mod"])
    }

    #[test]
    fn a_complete_plan_parses_and_maps_back_to_rows() {
        let plan = LoadPlan::parse(CHAIN_PLAN, &chain_request(), Producer::Preflight).unwrap();
        assert!(plan.ok);
        assert_eq!(
            map_order(plan.order.as_ref().unwrap(), &chain_request()).unwrap(),
            vec![1, 0, 2]
        );
        assert_eq!(plan.requirements.len(), 2);
        assert!(matches!(plan.conflicts[1], ConflictRow::Claims { .. }));
    }

    /// Every key is always present. A `null`-able key that is missing is as much
    /// a refusal as a required one.
    #[test]
    fn a_plan_missing_any_key_is_refused() {
        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v.as_object_mut().unwrap().remove("order");
        let err = LoadPlan::parse(&v.to_string(), &chain_request(), Producer::Preflight).unwrap_err();
        assert!(err.contains("order"), "{err}");

        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["items"][0].as_object_mut().unwrap().remove("quartermaster_range");
        assert!(LoadPlan::parse(&v.to_string(), &chain_request(), Producer::Preflight).is_err());

        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["findings"] = serde_json::json!([{ "code": "M0204", "severity": "warning",
            "message": "m", "items": [], "refs": [] }]);
        let err = LoadPlan::parse(&v.to_string(), &chain_request(), Producer::Preflight).unwrap_err();
        assert!(err.contains("fix"), "{err}");
    }

    #[test]
    fn an_extra_key_or_an_unknown_value_is_refused() {
        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["items"][0]["kind"] = serde_json::json!("shipment");
        assert!(LoadPlan::parse(&v.to_string(), &chain_request(), Producer::Preflight).is_err());

        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["conflicts"][0]["extra"] = serde_json::json!(1);
        assert!(LoadPlan::parse(&v.to_string(), &chain_request(), Producer::Preflight).is_err());

        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["requirements"][0]["status"] = serde_json::json!("probably_fine");
        assert!(LoadPlan::parse(&v.to_string(), &chain_request(), Producer::Preflight).is_err());
    }

    /// The field was renamed; a plan still carrying the old name is refused, not read.
    #[test]
    fn the_old_script_block_paths_name_is_refused() {
        let text = CHAIN_PLAN.replace("link_block_paths", "script_block_paths");
        let err = LoadPlan::parse(&text, &chain_request(), Producer::Preflight).unwrap_err();
        assert!(err.contains("script_block_paths") || err.contains("link_block_paths"), "{err}");
    }

    #[test]
    fn a_plan_of_another_format_is_refused() {
        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["format"] = serde_json::json!(2);
        let err = LoadPlan::parse(&v.to_string(), &chain_request(), Producer::Preflight).unwrap_err();
        assert!(err.contains("format 2"), "{err}");
    }

    #[test]
    fn a_plan_that_answers_a_different_request_is_refused() {
        let other = request(&["shipment:lua-bridge", "shipment:ess", "shipment:my-mod"]);
        assert!(LoadPlan::parse(CHAIN_PLAN, &other, Producer::Preflight).is_err());
        assert!(LoadPlan::parse(CHAIN_PLAN, &chain_request(), Producer::Link).is_err());
    }

    #[test]
    fn map_order_refuses_a_missing_extra_or_repeated_id() {
        let req = request(&["a", "b"]);
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(map_order(&ids(&["b", "a"]), &req).unwrap(), vec![1, 0]);
        assert!(map_order(&ids(&["a"]), &req).unwrap_err().contains("leaves out b"));
        assert!(map_order(&ids(&["a", "b", "c"]), &req).unwrap_err().contains("not requested"));
        assert!(map_order(&ids(&["a", "a"]), &req).unwrap_err().contains("twice"));
    }

    /// `ok` maps 1:1 to "no error finding". A plan that disagrees with itself is
    /// not one Modkit acts on.
    #[test]
    fn ok_must_agree_with_the_findings() {
        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["findings"] = serde_json::json!([{ "code": "M0204", "severity": "error",
            "message": "m", "items": [], "refs": [], "fix": null }]);
        assert!(LoadPlan::parse(&v.to_string(), &chain_request(), Producer::Preflight).is_err());
    }

    /// The build gate: a plan with `ok: false` is refused even though `order` is present
    /// (it is `null` only for a cycle), and the message names the rows, not the ids.
    #[test]
    fn a_plan_that_is_not_ok_refuses_the_build_naming_the_rows() {
        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["ok"] = serde_json::json!(false);
        v["requirements"][1]["status"] = serde_json::json!("version_unsatisfied");
        v["findings"] = serde_json::json!([{ "code": "M0204", "severity": "error",
            "message": "my-mod requires ess >=0.7, <1; the installed ess is 0.6.1",
            "items": ["shipment:my-mod", "shipment:ess"],
            "refs": [{ "section": "requirements", "index": 1 }], "fix": null }]);
        let plan = LoadPlan::parse(&v.to_string(), &chain_request(), Producer::Preflight).unwrap();
        assert!(plan.order.is_some(), "order is still present when ok is false");

        let row = |id: &str, name: &str| ShipmentRef {
            id: id.into(),
            name: name.into(),
            path: String::new(),
            slug: None,
            version: None,
            origin: crate::models::origin::Origin::local_unknown(),
            install_reason: crate::commands::shipment::InstallReason::User,
        };
        let err = plan
            .refuse_unless_ok(&[row("shipment:ess", "Ess"), row("shipment:my-mod", "My Mod")])
            .unwrap_err();
        assert!(err.contains("M0204"), "{err}");
        assert!(err.contains("My Mod, Ess"), "{err}");
        assert!(!err.contains("shipment:my-mod"), "ids stay local: {err}");
    }

    #[test]
    fn link_must_agree_with_preflight_on_order_and_edges() {
        let pre = LoadPlan::parse(CHAIN_PLAN, &chain_request(), Producer::Preflight).unwrap();
        let link_text = CHAIN_PLAN.replace(r#""producer": "preflight""#, r#""producer": "link""#);
        let link = LoadPlan::parse(&link_text, &chain_request(), Producer::Link).unwrap();
        assert!(assert_same_plan(&pre, &link).is_ok());

        let mut other = link.clone();
        other.order = Some(vec![
            "shipment:ess".into(),
            "shipment:lua-bridge".into(),
            "shipment:my-mod".into(),
        ]);
        assert!(assert_same_plan(&pre, &other).unwrap_err().contains("different order"));

        let mut other = link.clone();
        other.edges.pop();
        assert!(assert_same_plan(&pre, &other).unwrap_err().contains("edges"));

        // Link-time warnings (M0209) are allowed to differ.
        let mut warned = link;
        warned.findings.push(Finding {
            code: "M0209".into(),
            severity: Severity::Warning,
            message: "import(\"x\") does not resolve".into(),
            items: vec![],
            refs: vec![],
            fix: None,
        });
        assert!(assert_same_plan(&pre, &warned).is_ok());
    }

    #[test]
    fn a_missing_link_plan_is_a_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_link_plan(dir.path(), &chain_request()).unwrap_err();
        assert!(err.contains("wrote no readable"), "{err}");
    }

    #[test]
    fn a_missing_order_needs_a_cycle_finding_and_vice_versa() {
        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["order"] = serde_json::Value::Null;
        assert!(LoadPlan::parse(&v.to_string(), &chain_request(), Producer::Preflight).is_err());

        v["ok"] = serde_json::json!(false);
        v["findings"] = serde_json::json!([{ "code": "M0174", "severity": "error",
            "message": "cycle", "items": [], "refs": [], "fix": null }]);
        assert!(LoadPlan::parse(&v.to_string(), &chain_request(), Producer::Preflight).is_ok());
    }

    #[test]
    fn the_request_lists_rows_in_the_order_given() {
        let row = |id: &str| ShipmentRef {
            id: id.into(),
            name: id.into(),
            path: format!("/p/{id}"),
            slug: None,
            version: None,
            origin: crate::models::origin::Origin::local_unknown(),
            install_reason: crate::commands::shipment::InstallReason::User,
        };
        let req = LoadRequest::for_rows(&[row("shipment:b"), row("shipment:a")]);
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "format": 1, "items": [
                { "id": "shipment:b", "path": "/p/shipment:b" },
                { "id": "shipment:a", "path": "/p/shipment:a" } ] })
        );
    }
}
