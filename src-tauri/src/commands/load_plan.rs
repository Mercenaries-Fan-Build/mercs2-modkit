//! `qm preflight`: the request Modkit writes and the `load-plan.json` qm writes back.
//!
//! Quartermaster owns both formats. The plan's types are qm's own, from
//! [`mercs2_formats::load_plan`], the one definition qm writes with; this module re-exports them.
//! The request (`load-request.json`) has no shared definition, so [`LoadRequest`] mirrors qm's,
//! and where the two ever disagree qm wins and this file is wrong.
//!
//! # Refuse, never guess
//!
//! * Every key is always present in a plan. Keys that don't apply are `null`, not missing. The
//!   shared types deny unknown fields and read a missing nullable key as an error.
//! * Every closed set is an enum, so an unknown value fails to parse. The shared finding types
//!   also serve qm's lint and range reports, so they admit severities (`info`, `hang`) and
//!   sections (`contributions`, `ranges`) a plan never carries; [`parse_plan`] refuses those.
//! * `format` is each file's own version: [`PLAN_FORMAT`] (3) for `load-plan.json`,
//!   [`REQUEST_FORMAT`] (1) for `load-request.json`. The manifest format is a different number.
//!   A plan of another format is refused before it is parsed, naming the remedy: the installed
//!   Workshop toolset's qm is older or newer than this Modkit reads.
//!
//! # What Modkit does with it
//!
//! Modkit runs `qm preflight` before every Shipment build and refuses to build when `ok` is
//! false. The dependency resolver and the removal cascade read the plan's `requirements` rather
//! than parsing any manifest's `load` table themselves: Modkit never parses `load` itself, and
//! `required_by` is derived from the latest plan each time, so it can't drift.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Deserializer, Serialize};

pub use mercs2_formats::load_plan::{
    ConflictRow, DataFile, Finding, FindingSeverity, LoadPlan, PlanItem, PluginEntry, Producer,
    RequirementKind, RequirementRow, RequirementStatus, Section, PLAN_FILE, PLAN_FORMAT,
};

use super::proc::{run_streaming, NoWindow};
use super::shipment::ShipmentRef;

/// The `format` of `load-request.json`.
pub const REQUEST_FORMAT: u32 = 1;

/// The request file's name inside a work dir. `qm link --request` is given the same file
/// preflight was given.
pub const REQUEST_FILE: &str = "load-request.json";

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
            format: REQUEST_FORMAT,
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

/// The cycle code: `order` is `null` exactly when a finding carries it.
const CYCLE_CODE: &str = "M0174";

/// Parse a plan and check it against the request it answers.
///
/// The `format` is read first, on its own, so a plan of another format is refused by number and
/// with the remedy, rather than by whichever key the formats happen to differ in.
pub fn parse_plan(text: &str, request: &LoadRequest, producer: Producer) -> Result<LoadPlan, String> {
    #[derive(Deserialize)]
    struct Head {
        format: u32,
    }
    let head: Head = serde_json::from_str(text)
        .map_err(|e| format!("qm wrote a load-plan.json Modkit cannot read: {e}"))?;
    if head.format != PLAN_FORMAT {
        return Err(format!(
            "load-plan.json has format {}; Modkit reads only format {PLAN_FORMAT}, which the \
             Workshop toolset v1.19.0 (qm 6.0.0) writes. Update the Workshop toolset in Modkit.",
            head.format
        ));
    }
    let plan: LoadPlan = serde_json::from_str(text)
        .map_err(|e| format!("qm wrote a load-plan.json Modkit cannot read: {e}"))?;
    validate(&plan, request, producer)?;
    Ok(plan)
}

/// The invariants of the plan format that Modkit relies on. Any breach is a hard error: a plan
/// that doesn't answer the request it was given can't be mapped back to rows.
fn validate(plan: &LoadPlan, request: &LoadRequest, producer: Producer) -> Result<(), String> {
    plan.check().map_err(|e| format!("load-plan.json: {e}"))?;
    if plan.producer != producer {
        return Err(format!(
            "load-plan.json was written by {:?}, expected {producer:?}",
            plan.producer
        ));
    }
    if plan.items.len() != request.items.len() {
        return Err(format!(
            "load-plan.json has {} items for a request of {}",
            plan.items.len(),
            request.items.len()
        ));
    }
    // The shared finding types also carry lint's and the range report's; a plan holds only
    // warnings and errors, pointing into its own five sections.
    for f in &plan.findings {
        if !matches!(f.severity, FindingSeverity::Warning | FindingSeverity::Error) {
            return Err(format!(
                "load-plan.json finding {} has severity {}, which a load plan never carries",
                f.code,
                f.severity.as_str()
            ));
        }
        if let Some(r) = f.refs.iter().find(|r| {
            !matches!(
                r.section,
                Section::Items
                    | Section::Edges
                    | Section::Requirements
                    | Section::Conflicts
                    | Section::Supersedes
            )
        }) {
            return Err(format!(
                "load-plan.json finding {} points into {:?}, which is not a load plan section",
                f.code, r.section
            ));
        }
    }
    for (i, (item, want)) in plan.items.iter().zip(&request.items).enumerate() {
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
    let has_cycle = plan.findings.iter().any(|f| f.code == CYCLE_CODE);
    match &plan.order {
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
    let has_error = plan.findings.iter().any(|f| f.severity == FindingSeverity::Error);
    if plan.ok == has_error {
        return Err(format!(
            "load-plan.json says ok = {} but {} an error finding",
            plan.ok,
            if has_error { "carries" } else { "carries no" }
        ));
    }
    for r in &plan.requirements {
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

/// What Modkit reads out of a parsed plan.
pub trait PlanExt {
    /// Every data file any item in the set edits.
    fn data_files(&self) -> BTreeSet<DataFile>;
    /// The plan's findings as text, with each request id replaced by its row's name. Used for
    /// the refusal message, which the player reads.
    fn describe_findings(&self, names: &HashMap<String, String>) -> String;
    /// Refuse when `ok` is false.
    fn refuse_unless_ok(&self, rows: &[ShipmentRef]) -> Result<(), String>;
    /// Every `shipment`-kind and `capability`-kind requirement, grouped by consumer id.
    fn requirements_by_consumer(&self) -> BTreeMap<&str, Vec<&RequirementRow>>;
}

impl PlanExt for LoadPlan {
    fn data_files(&self) -> BTreeSet<DataFile> {
        self.items.iter().flat_map(|i| i.data_files.iter().copied()).collect()
    }

    fn describe_findings(&self, names: &HashMap<String, String>) -> String {
        self.findings
            .iter()
            .map(|f| {
                let sev = f.severity.as_str();
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

    fn refuse_unless_ok(&self, rows: &[ShipmentRef]) -> Result<(), String> {
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

    fn requirements_by_consumer(&self) -> BTreeMap<&str, Vec<&RequirementRow>> {
        let mut out: BTreeMap<&str, Vec<&RequirementRow>> = BTreeMap::new();
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
/// A row whose folder is missing is refused here, naming every such row, before qm runs: qm's
/// own refusal names only the row id.
///
/// Exit 0 and 1 both write a plan, which is read and checked here; exit 0 must carry `ok: true`
/// and exit 1 `ok: false`. Exit 2 means qm could not run, and its stderr is the error. Any other
/// exit, or a plan missing after exit 0 or 1, is an error too. The caller decides what `ok:
/// false` means for it: a build refuses (see [`PlanExt::refuse_unless_ok`]), while the resolver
/// and the removal cascade still read the requirement data.
///
/// Each line qm prints is handed to `on_line` as it arrives.
pub fn run_preflight(
    qm: &Path,
    rows: &[ShipmentRef],
    game: &OsStr,
    work: &Path,
    on_line: &mut dyn FnMut(&str),
) -> Result<LoadPlan, String> {
    let missing = super::shipment::missing_rows(rows);
    if !missing.is_empty() {
        return Err(super::shipment::missing_rows_error(&missing));
    }
    let request = LoadRequest::for_rows(rows);
    let request_path = work.join(REQUEST_FILE);
    let text = serde_json::to_string_pretty(&request)
        .map_err(|e| format!("building load-request.json: {e}"))?;
    std::fs::write(&request_path, text)
        .map_err(|e| format!("writing {}: {e}", request_path.display()))?;

    let output = run_streaming(
        Command::new(qm)
            .arg("preflight")
            .arg("--request")
            .arg(&request_path)
            .arg("--out")
            .arg(work)
            .arg("--game")
            .arg(game)
            .no_window(),
        on_line,
    )
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
    let plan = parse_plan(&text, &request, Producer::Preflight)?;
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
    parse_plan(&text, request, Producer::Link)
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
            format: REQUEST_FORMAT,
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
      "format": 3, "producer": "preflight", "quartermaster": "6.0.0", "ok": true,
      "order": ["shipment:lua-bridge", "shipment:ess", "shipment:my-mod"],
      "items": [
        { "id": "shipment:ess", "requested": 0, "resolved": 1, "held_back_by": 0,
          "name": "ess", "version": "0.7.0", "manifest_format": 2, "quartermaster_range": null,
          "provides": [], "plugins": [], "runtime_dlls": [], "placed_files": [], "data_files": [] },
        { "id": "shipment:lua-bridge", "requested": 1, "resolved": 0, "held_back_by": null,
          "name": "lua-bridge", "version": "1.0.0", "manifest_format": 2, "quartermaster_range": null,
          "provides": [],
          "plugins": [ { "contribution": 0, "file_name": "lua_bridge_DEV.asi", "source": "lua_bridge_DEV.asi",
                         "relative": "scripts/lua_bridge_DEV.asi", "sha256": "00" } ],
          "runtime_dlls": [],
          "placed_files": [ { "contribution": 1, "file_name": "lua_bridge_DEV.ini", "source": "lua_bridge_DEV.ini",
                              "destination": "game_folder", "dest": "scripts", "relative": "scripts/lua_bridge_DEV.ini" } ], "data_files": [] },
        { "id": "shipment:my-mod", "requested": 2, "resolved": 2, "held_back_by": null,
          "name": "my-mod", "version": "1.0.0", "manifest_format": 2, "quartermaster_range": null,
          "provides": [], "plugins": [], "runtime_dlls": [], "placed_files": [], "data_files": ["data/shader3.bin"] }
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
      "link_file_paths": ["data/shader3.bin"],
      "libraries": null,
      "findings": []
    }"#;

    pub(crate) fn chain_request() -> LoadRequest {
        request(&["shipment:ess", "shipment:lua-bridge", "shipment:my-mod"])
    }

    /// What `qm preflight` and `qm link` (qm 6.0.0, the Workshop Tools v1.19.0 release) wrote for
    /// m2-sdk, lua-bridge, Ess and mercs2-language, requested in the order below.
    const QM_PREFLIGHT_PLAN: &str = include_str!("../../fixtures/qm-6.0.0/preflight-load-plan.json");
    const QM_LINK_PLAN: &str = include_str!("../../fixtures/qm-6.0.0/link-load-plan.json");

    fn qm_request() -> LoadRequest {
        request(&["shipment:ess", "shipment:lua-bridge", "shipment:m2-sdk", "shipment:mercs2-language"])
    }

    /// The plans the released qm writes parse, map back to the request, and agree with each other.
    #[test]
    fn the_plans_released_qm_writes_parse_and_agree() {
        let pre = parse_plan(QM_PREFLIGHT_PLAN, &qm_request(), Producer::Preflight).unwrap();
        let link = parse_plan(QM_LINK_PLAN, &qm_request(), Producer::Link).unwrap();
        assert!(pre.ok && link.ok);
        assert_eq!(map_order(pre.order.as_ref().unwrap(), &qm_request()).unwrap(), vec![2, 1, 0, 3]);
        assert_same_plan(&pre, &link).unwrap();
    }

    #[test]
    fn a_complete_plan_parses_and_maps_back_to_rows() {
        let plan = parse_plan(CHAIN_PLAN, &chain_request(), Producer::Preflight).unwrap();
        assert!(plan.ok);
        assert_eq!(
            map_order(plan.order.as_ref().unwrap(), &chain_request()).unwrap(),
            vec![1, 0, 2]
        );
        assert_eq!(plan.requirements.len(), 2);
        assert!(matches!(plan.conflicts[1], ConflictRow::Claims(_)));
    }

    /// Every key is always present. A `null`-able key that is missing is as much
    /// a refusal as a required one.
    #[test]
    fn a_plan_missing_any_key_is_refused() {
        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v.as_object_mut().unwrap().remove("order");
        let err = parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).unwrap_err();
        assert!(err.contains("order"), "{err}");

        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["items"][0].as_object_mut().unwrap().remove("quartermaster_range");
        assert!(parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).is_err());

        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["findings"] = serde_json::json!([{ "code": "M0204", "severity": "warning",
            "message": "m", "items": [], "refs": [] }]);
        let err = parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).unwrap_err();
        assert!(err.contains("fix"), "{err}");
    }

    #[test]
    fn an_extra_key_or_an_unknown_value_is_refused() {
        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["items"][0]["kind"] = serde_json::json!("shipment");
        assert!(parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).is_err());

        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["conflicts"][0]["extra"] = serde_json::json!(1);
        assert!(parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).is_err());

        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["requirements"][0]["status"] = serde_json::json!("probably_fine");
        assert!(parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).is_err());
    }

    #[test]
    fn a_plugin_entry_with_touches_and_signature_guard_parses() {
        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["items"][1]["plugins"][0]["touches"] = serde_json::json!(["0x004B1180", "0x0063DA1F"]);
        v["items"][1]["plugins"][0]["signature_guard"] = serde_json::json!({
            "0x004B1180": "8B 44 24 04 85 C0",
        });
        let plan =
            parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).unwrap();
        let plugin = &plan.items[1].plugins[0];
        assert_eq!(plugin.touches, vec!["0x004B1180", "0x0063DA1F"]);
        assert_eq!(plugin.signature_guard.get("0x004B1180").map(String::as_str),
                   Some("8B 44 24 04 85 C0"));

        let plan = parse_plan(CHAIN_PLAN, &chain_request(), Producer::Preflight).unwrap();
        assert!(plan.items[1].plugins[0].touches.is_empty());
        assert!(plan.items[1].plugins[0].signature_guard.is_empty());
    }

    /// qm's runtime DLL entry has never carried `touches` or `signature_guard` (only a plugin entry
    /// does), so a runtime DLL entry with either is not a plan qm wrote, and is refused.
    #[test]
    fn a_runtime_dll_entry_parses_and_one_with_touches_or_signature_guard_is_refused() {
        let dll = serde_json::json!({
            "contribution": 2, "file_name": "x.dll", "source": "x.dll",
            "relative": "x.dll", "sha256": "00",
        });
        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["items"][1]["runtime_dlls"] = serde_json::json!([dll.clone()]);
        let plan = parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).unwrap();
        assert_eq!(plan.items[1].runtime_dlls[0].file_name, "x.dll");

        for (key, value) in [
            ("touches", serde_json::json!(["0x004B1180"])),
            ("signature_guard", serde_json::json!({ "0x004B1180": "8B 44 24 04" })),
        ] {
            let mut entry = dll.clone();
            entry[key] = value;
            v["items"][1]["runtime_dlls"] = serde_json::json!([entry]);
            let err = parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).unwrap_err();
            assert!(err.contains(key), "{key}: {err}");
        }
    }

    /// Each item's `data_files` and the plan's `link_file_paths` read as the closed data-file
    /// set; the union says, before any build, which stores the set edits.
    #[test]
    fn data_files_and_link_file_paths_parse() {
        let plan = parse_plan(CHAIN_PLAN, &chain_request(), Producer::Preflight).unwrap();
        assert_eq!(plan.link_file_paths, vec![DataFile::Shader3]);
        assert_eq!(plan.items[2].data_files, vec![DataFile::Shader3]);
        assert!(plan.items[0].data_files.is_empty());
        assert_eq!(plan.data_files().into_iter().collect::<Vec<_>>(), vec![DataFile::Shader3]);
    }

    /// A path outside the closed set, or a plan without either key, is refused.
    #[test]
    fn a_data_file_outside_the_set_or_a_missing_key_is_refused() {
        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["items"][2]["data_files"] = serde_json::json!(["data/shader4.bin"]);
        let err = parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).unwrap_err();
        assert!(err.contains("unknown variant `data/shader4.bin`"), "{err}");

        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["link_file_paths"] = serde_json::json!(["data/Shader3Low.bin"]);
        assert!(parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).is_err());

        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v.as_object_mut().unwrap().remove("link_file_paths");
        assert!(parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).is_err());

        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["items"][1].as_object_mut().unwrap().remove("data_files");
        assert!(parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).is_err());
    }

    /// The field was renamed; a plan still carrying the old name is refused, not read.
    #[test]
    fn the_old_script_block_paths_name_is_refused() {
        let text = CHAIN_PLAN.replace("link_block_paths", "script_block_paths");
        let err = parse_plan(&text, &chain_request(), Producer::Preflight).unwrap_err();
        assert!(err.contains("script_block_paths") || err.contains("link_block_paths"), "{err}");
    }

    #[test]
    fn a_plan_of_another_format_is_refused() {
        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["format"] = serde_json::json!(1);
        let err = parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).unwrap_err();
        assert!(err.contains("format 1"), "{err}");
    }

    /// The plan an older toolset's qm writes (format 2, no `libraries`) is refused by its format,
    /// naming the remedy, not by the key it lacks.
    #[test]
    fn a_format_2_plan_is_refused_naming_the_toolset_update() {
        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["format"] = serde_json::json!(2);
        v.as_object_mut().unwrap().remove("libraries");
        let err = parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).unwrap_err();
        assert!(err.contains("format 2"), "{err}");
        assert!(err.contains("Update the Workshop toolset"), "{err}");
    }

    /// `libraries` is always present: `null` from preflight, a list from link.
    #[test]
    fn libraries_is_null_from_preflight_listed_from_link_and_never_missing() {
        let pre = parse_plan(QM_PREFLIGHT_PLAN, &qm_request(), Producer::Preflight).unwrap();
        assert_eq!(pre.libraries, None);
        let link = parse_plan(QM_LINK_PLAN, &qm_request(), Producer::Link).unwrap();
        let modules: Vec<&str> = link.libraries.as_ref().unwrap().iter().map(|l| l.module.as_str()).collect();
        assert_eq!(modules, vec!["ess", "ess_names"]);

        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v.as_object_mut().unwrap().remove("libraries");
        let err = parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).unwrap_err();
        assert!(err.contains("libraries"), "{err}");
    }

    /// The shared finding types also serve lint and the range report. A plan carries only warning
    /// and error findings, pointing into its own five sections; anything else is refused.
    #[test]
    fn a_finding_a_plan_never_carries_is_refused() {
        for severity in ["info", "hang"] {
            let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
            v["findings"] = serde_json::json!([{ "code": "M0204", "severity": severity,
                "message": "m", "items": [], "refs": [], "fix": null }]);
            let err = parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).unwrap_err();
            assert!(err.contains(&format!("severity {severity}")), "{err}");
        }
        for section in ["contributions", "ranges"] {
            let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
            v["findings"] = serde_json::json!([{ "code": "M0204", "severity": "warning",
                "message": "m", "items": [], "refs": [{ "section": section, "index": 0 }], "fix": null }]);
            let err = parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).unwrap_err();
            assert!(err.contains("not a load plan section"), "{section}: {err}");
        }
    }

    /// A conflict row whose `source` names the other shape is refused.
    #[test]
    fn a_conflict_row_whose_source_names_the_other_shape_is_refused() {
        let mut v: serde_json::Value = serde_json::from_str(CHAIN_PLAN).unwrap();
        v["conflicts"][0]["source"] = serde_json::json!("claims");
        let err = parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).unwrap_err();
        assert!(err.contains("conflicts[0]"), "{err}");
    }

    #[test]
    fn a_plan_that_answers_a_different_request_is_refused() {
        let other = request(&["shipment:lua-bridge", "shipment:ess", "shipment:my-mod"]);
        assert!(parse_plan(CHAIN_PLAN, &other, Producer::Preflight).is_err());
        assert!(parse_plan(CHAIN_PLAN, &chain_request(), Producer::Link).is_err());
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
        assert!(parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).is_err());
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
        let plan = parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).unwrap();
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
        let pre = parse_plan(CHAIN_PLAN, &chain_request(), Producer::Preflight).unwrap();
        let link_text = CHAIN_PLAN.replace(r#""producer": "preflight""#, r#""producer": "link""#);
        let link = parse_plan(&link_text, &chain_request(), Producer::Link).unwrap();
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
            severity: FindingSeverity::Warning,
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
        assert!(parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).is_err());

        v["ok"] = serde_json::json!(false);
        v["findings"] = serde_json::json!([{ "code": "M0174", "severity": "error",
            "message": "cycle", "items": [], "refs": [], "fix": null }]);
        assert!(parse_plan(&v.to_string(), &chain_request(), Producer::Preflight).is_ok());
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

    /// A row whose folder is missing is refused by name and path before qm runs: the qm path
    /// here does not exist, so reaching qm would fail with "running qm preflight" instead.
    #[test]
    fn a_missing_folder_is_refused_before_qm_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let here = tmp.path().join("here");
        std::fs::create_dir(&here).unwrap();
        let row = |id: &str, path: &Path| ShipmentRef {
            id: format!("shipment:{id}"),
            name: id.into(),
            path: path.to_string_lossy().into(),
            slug: Some(id.into()),
            version: None,
            origin: crate::models::origin::Origin::local_unknown(),
            install_reason: crate::commands::shipment::InstallReason::User,
        };
        let gone = tmp.path().join("gone");
        let rows = [row("here", &here), row("gone", &gone)];
        let mut printed: Vec<String> = Vec::new();
        let e = run_preflight(
            &tmp.path().join("no-qm"),
            &rows,
            OsStr::new("vz.wad"),
            tmp.path(),
            &mut |l| printed.push(l.to_string()),
        )
        .unwrap_err();
        assert!(printed.is_empty(), "{printed:?}");
        assert!(e.starts_with("The files of 1 Shipment"), "{e}");
        assert!(e.contains(&format!("gone: {}", gone.display())), "{e}");
        assert!(!e.contains("here:"), "{e}");
    }
}
