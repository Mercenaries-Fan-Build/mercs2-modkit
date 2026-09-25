//! Shipments that other Shipments require: installing them, and removing them.
//!
//! # Where requirement data comes from
//!
//! Modkit never parses a manifest's `load` table itself. Requirement data has two
//! sources, and both were parsed by qm:
//! * for **installed** rows, the latest `qm preflight` plan's `requirements`, so
//!   `required_by` is derived each time and can't drift;
//! * for a **release not yet downloaded**, the manifest mercs.ink relays with the release, which
//!   mercs.ink gets by running qm rather than by mirroring qm's rules.
//!
//! # What is resolved, and what is not
//!
//! * `shipment` requirements (a bare name, or `{ shipment, version }`) are resolved: the
//!   resolver installs the highest release that satisfies every range on that name, and never
//!   goes below an installed version.
//! * `capability` requirements are **not** auto-installed. Which provider to install is not
//!   decided, and Modkit never infers an undeclared need. An unmet capability is reported by
//!   preflight as M0204.
//! * Re-resolving every dependency row on every build, and excluding prereleases, are not
//!   done here.
//!
//! # Removal
//!
//! Removing a Shipment removes every installed Shipment that requires it, transitively; a
//! capability dependent goes only when the last provider of that capability goes. Then
//! `dependency` rows that nothing still requires are removed as orphans, repeated to a fixed
//! point. The whole chain is computed first and shown to the player, who confirms once; the
//! confirmed set is exactly what is removed.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use tauri::Window;

use super::load_plan::{LoadPlan, Requirement, RequirementKind};
use super::managed::trash;
use super::paths::staging_dir;
use super::shipment::{preflight_rows, InstallReason, ShipmentRef};
use crate::models::origin::OriginSource;

// ---------------------------------------------------------------------------------------
// Requirements as the resolver sees them
// ---------------------------------------------------------------------------------------

/// One `load.requires` entry of a release mercs.ink relays, in qm's format-2 spellings.
///
/// Each object form is `deny_unknown_fields`, so a shape qm's model does not have (the retired
/// `{ url, sha256 }`, the rejected `{ name, version }`) matches nothing and is an error.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RelayedRequirement {
    Shipment(String),
    ShipmentRange(ShipmentRangeReq),
    Capability(CapabilityReq),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShipmentRangeReq {
    pub shipment: String,
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityReq {
    pub capability: String,
}

/// A `shipment`-kind requirement: a target name and its range (`None` = any version).
#[derive(Debug, Clone)]
pub struct ShipmentNeed {
    pub target: String,
    pub range: Option<VersionReq>,
    /// The range as written, for messages.
    pub range_text: Option<String>,
}

impl ShipmentNeed {
    fn new(target: &str, range: Option<&str>) -> Result<Self, String> {
        let parsed = match range {
            Some(r) => Some(
                VersionReq::parse(r)
                    .map_err(|e| format!("the range \"{r}\" on {target} is not a semver range: {e}"))?,
            ),
            None => None,
        };
        Ok(ShipmentNeed {
            target: target.to_string(),
            range: parsed,
            range_text: range.map(str::to_string),
        })
    }

    fn admits(&self, v: &Version) -> bool {
        self.range.as_ref().is_none_or(|r| r.matches(v))
    }

    fn describe(&self) -> String {
        match &self.range_text {
            Some(r) => format!("{} {r}", self.target),
            None => format!("{} (any version)", self.target),
        }
    }
}

/// The `shipment` requirements in a relayed manifest's `load` table. A missing `load` or a
/// `load` without `requires` means none; any requirement of a shape qm does not have is an error.
pub fn relayed_shipment_needs(
    load: Option<&serde_json::Value>,
    what: &str,
) -> Result<Vec<ShipmentNeed>, String> {
    let Some(load) = load else { return Ok(Vec::new()) };
    if load.is_null() {
        return Ok(Vec::new());
    }
    let Some(requires) = load.get("requires") else { return Ok(Vec::new()) };
    if requires.is_null() {
        return Ok(Vec::new());
    }
    let list: Vec<RelayedRequirement> = serde_json::from_value(requires.clone()).map_err(|e| {
        format!("{what}'s manifest, as mercs.ink relays it, has a load.requires Modkit cannot read: {e}")
    })?;
    let mut out = Vec::new();
    for r in list {
        match r {
            RelayedRequirement::Shipment(name) => out.push(ShipmentNeed::new(&name, None)?),
            RelayedRequirement::ShipmentRange(s) => {
                out.push(ShipmentNeed::new(&s.shipment, Some(&s.version))?)
            }
            // Not auto-installed; preflight reports an unmet one (module docs).
            RelayedRequirement::Capability(_) => {}
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// The resolver (pure)
// ---------------------------------------------------------------------------------------

/// One release of a Shipment the resolver may pick.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub version: Version,
    /// The release's own version string, as the registry names it, for fetching it.
    pub release_version: String,
    pub needs: Vec<ShipmentNeed>,
}

/// An installed row, as the latest preflight plan names it.
#[derive(Debug, Clone)]
pub struct InstalledRow {
    pub id: String,
    pub name: String,
    pub version: Version,
    /// Installed through mercs.ink, so the resolver may update it. A row staged from a folder
    /// can only be checked, never replaced.
    pub from_registry: bool,
}

/// A requirement an installed row declares, as the latest preflight plan reports it.
#[derive(Debug, Clone)]
pub struct InstalledNeed {
    pub declarer: String,
    pub need: ShipmentNeed,
}

/// What to do about one required Shipment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Install,
    Update { from: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub name: String,
    pub release_version: String,
    pub action: Action,
}

/// One pass of the resolver: either it needs a Shipment's releases it has not seen yet, or it
/// is done.
#[derive(Debug)]
pub enum Step {
    NeedReleases(String),
    Done(Vec<Pick>),
}

/// How many passes before the resolver gives up. Each pass either changes a pick or finishes, and
/// a pick only moves when a newly added range excludes it, so a real set settles in a few.
const MAX_PASSES: usize = 64;

/// Resolve `root`'s requirements transitively against what is installed.
///
/// * `root` is the Shipment the player asked for; its version is theirs to choose, so it is never
///   picked here, and the ranges others place on it are preflight's to check.
/// * `installed_needs` must not include requirements the root's own current row declares; those
///   are replaced by `root_needs`.
/// * For each required name, the pick is the highest candidate that satisfies **every** range on
///   that name and is not below the installed version. An installed row that already is that
///   version is kept. A row staged from a folder is kept if it satisfies every range and is an
///   error otherwise.
/// * Returns [`Step::NeedReleases`] for the first required name whose releases are not in
///   `catalog`; the caller fetches them and calls again.
pub fn resolve(
    root: &str,
    root_needs: &[ShipmentNeed],
    installed: &[InstalledRow],
    installed_needs: &[InstalledNeed],
    catalog: &BTreeMap<String, Vec<Candidate>>,
) -> Result<Step, String> {
    let installed_by_name: HashMap<&str, &InstalledRow> =
        installed.iter().map(|r| (r.name.as_str(), r)).collect();

    // name -> (index into catalog[name]) of the current pick, when it is a new or updated release.
    let mut chosen: BTreeMap<String, usize> = BTreeMap::new();

    for _ in 0..MAX_PASSES {
        // Every range currently in force, by target, with who declares it.
        let mut ranges: BTreeMap<&str, Vec<(String, &ShipmentNeed)>> = BTreeMap::new();
        for n in root_needs {
            ranges.entry(n.target.as_str()).or_default().push((root.to_string(), n));
        }
        for n in installed_needs {
            // An installed row being replaced by a new release declares that release's needs
            // instead of its own.
            if chosen.contains_key(&n.declarer) {
                continue;
            }
            ranges
                .entry(n.need.target.as_str())
                .or_default()
                .push((n.declarer.clone(), &n.need));
        }
        for (name, &i) in &chosen {
            let cand = &catalog[name][i];
            for n in &cand.needs {
                ranges
                    .entry(n.target.as_str())
                    .or_default()
                    .push((format!("{name} {}", cand.version), n));
            }
        }

        let mut next: BTreeMap<String, usize> = BTreeMap::new();
        for (&target, declared) in &ranges {
            if target == root {
                continue;
            }
            let why = || {
                declared
                    .iter()
                    .map(|(who, n)| format!("{who} requires {}", n.describe()))
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            let installed_row = installed_by_name.get(target).copied();

            if let Some(row) = installed_row.filter(|r| !r.from_registry) {
                if declared.iter().all(|(_, n)| n.admits(&row.version)) {
                    continue;
                }
                return Err(format!(
                    "{target} {} is installed from a folder, not from mercs.ink, and it does not satisfy what is required ({}). Update or remove that folder's Shipment; Modkit will not replace it.",
                    row.version,
                    why()
                ));
            }

            let Some(cands) = catalog.get(target) else {
                return Ok(Step::NeedReleases(target.to_string()));
            };
            let satisfying: Vec<(usize, &Candidate)> = cands
                .iter()
                .enumerate()
                .filter(|(_, c)| declared.iter().all(|(_, n)| n.admits(&c.version)))
                .collect();
            let floor = installed_row.map(|r| &r.version);
            let best = satisfying
                .iter()
                .filter(|(_, c)| floor.is_none_or(|f| &c.version >= f))
                .max_by(|a, b| a.1.version.cmp(&b.1.version));

            match best {
                Some(&(i, c)) => {
                    if floor == Some(&c.version) {
                        // Already installed at the pick: keep it, and its own needs are already
                        // in `installed_needs`.
                        continue;
                    }
                    next.insert(target.to_string(), i);
                }
                None => {
                    if let (Some(f), false) = (floor, satisfying.is_empty()) {
                        return Err(format!(
                            "{target} {f} is installed, and the only releases satisfying what is required are older ({}). Modkit never downgrades on its own; remove {target} first if you mean to roll it back.",
                            why()
                        ));
                    }
                    return Err(format!(
                        "No release of {target} on mercs.ink satisfies everything required of it ({}).",
                        why()
                    ));
                }
            }
        }

        if next == chosen {
            let picks = chosen
                .iter()
                .map(|(name, &i)| {
                    let c = &catalog[name][i];
                    Pick {
                        name: name.clone(),
                        release_version: c.release_version.clone(),
                        action: match installed_by_name.get(name.as_str()) {
                            Some(row) => Action::Update { from: row.version.to_string() },
                            None => Action::Install,
                        },
                    }
                })
                .collect();
            return Ok(Step::Done(picks));
        }
        chosen = next;
    }
    Err(format!(
        "Resolving the requirements of {root} did not settle after {MAX_PASSES} passes; the ranges its dependencies declare keep excluding each other's picks."
    ))
}

/// The installed rows and their requirements, read off a preflight plan.
///
/// `root` names the Shipment being installed: requirements its current row declares are dropped,
/// because the release being installed replaces them.
pub fn installed_from_plan(
    plan: &LoadPlan,
    rows: &[ShipmentRef],
    root: &str,
) -> Result<(Vec<InstalledRow>, Vec<InstalledNeed>), String> {
    let by_id: HashMap<&str, &ShipmentRef> = rows.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut installed = Vec::new();
    let mut names: HashMap<&str, &str> = HashMap::new();
    for item in &plan.items {
        let row = by_id
            .get(item.id.as_str())
            .ok_or_else(|| format!("the preflight plan names {}, which is not a library row", item.id))?;
        let version = Version::parse(&item.version).map_err(|e| {
            format!("qm reported {} at version \"{}\", which is not semver: {e}", item.name, item.version)
        })?;
        names.insert(item.id.as_str(), item.name.as_str());
        installed.push(InstalledRow {
            id: item.id.clone(),
            name: item.name.clone(),
            version,
            from_registry: row.origin.source == OriginSource::Registry,
        });
    }
    let mut needs = Vec::new();
    for r in &plan.requirements {
        if r.kind != RequirementKind::Shipment {
            continue;
        }
        let declarer = names
            .get(r.consumer.as_str())
            .ok_or_else(|| format!("the preflight plan's requirement names {}, which is not one of its items", r.consumer))?;
        if *declarer == root {
            continue;
        }
        needs.push(InstalledNeed {
            declarer: declarer.to_string(),
            need: ShipmentNeed::new(&r.target, r.range.as_deref())?,
        });
    }
    Ok((installed, needs))
}

// ---------------------------------------------------------------------------------------
// Removal: cascade and orphans (pure)
// ---------------------------------------------------------------------------------------

/// Why a Shipment is in the removal set: the requirement of its that pulled it in.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PulledBy {
    /// `shipment` or `capability`.
    pub kind: String,
    /// The required Shipment name or capability token.
    pub target: String,
    pub range: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cascaded {
    pub shipment: ShipmentRef,
    pub pulled_by: PulledBy,
}

/// The full chain a removal takes, shown to the player before anything is removed.
#[derive(Debug, Clone, Serialize)]
pub struct RemovalPlan {
    /// The Shipment the player asked to remove.
    pub removed: ShipmentRef,
    /// Everything the cascade takes, with the requirement that pulled each one in.
    pub cascade: Vec<Cascaded>,
    /// The dependencies the orphan pass then removes.
    pub orphans: Vec<ShipmentRef>,
}

impl RemovalPlan {
    /// Every row the plan removes, in the order it was computed.
    pub fn all(&self) -> Vec<&ShipmentRef> {
        std::iter::once(&self.removed)
            .chain(self.cascade.iter().map(|c| &c.shipment))
            .chain(self.orphans.iter())
            .collect()
    }
}

fn pulled_by(r: &Requirement) -> PulledBy {
    PulledBy {
        kind: match r.kind {
            RequirementKind::Shipment => "shipment".into(),
            RequirementKind::Capability => "capability".into(),
        },
        target: r.target.clone(),
        range: r.range.clone(),
    }
}

/// Compute the removal chain for `target_id` from a preflight plan over `rows`.
///
/// Cascade: X joins the set when it has a `shipment` requirement one of whose providers is
/// in the set, or a `capability` requirement whose every provider is in the set (so removing
/// one of several providers takes nothing else). Repeated until the set stops growing. `user`
/// rows are included.
///
/// Orphans: of the rows left, a `dependency` row that no other remaining row's requirement
/// names as a provider is an orphan; removed, and repeated to a fixed point. `user` rows never
/// are.
pub fn plan_removal(
    target_id: &str,
    rows: &[ShipmentRef],
    plan: &LoadPlan,
) -> Result<RemovalPlan, String> {
    let removed = rows
        .iter()
        .find(|r| r.id == target_id)
        .ok_or_else(|| format!("{target_id} is not in the library"))?
        .clone();
    let row_ids: HashSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    for item in &plan.items {
        if !row_ids.contains(item.id.as_str()) {
            return Err(format!("the preflight plan names {}, which is not a library row", item.id));
        }
    }

    let by_consumer = plan.requirements_by_consumer();
    let mut set: BTreeSet<String> = BTreeSet::from([removed.id.clone()]);
    let mut cascade: Vec<Cascaded> = Vec::new();

    loop {
        let mut grew = false;
        for row in rows {
            if set.contains(&row.id) {
                continue;
            }
            let Some(reqs) = by_consumer.get(row.id.as_str()) else { continue };
            let pulled = reqs.iter().find(|r| match r.kind {
                RequirementKind::Shipment => r.providers.iter().any(|p| set.contains(p)),
                RequirementKind::Capability => {
                    !r.providers.is_empty() && r.providers.iter().all(|p| set.contains(p))
                }
            });
            if let Some(r) = pulled {
                set.insert(row.id.clone());
                cascade.push(Cascaded { shipment: row.clone(), pulled_by: pulled_by(r) });
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    let mut remaining: BTreeSet<String> = rows
        .iter()
        .map(|r| r.id.clone())
        .filter(|id| !set.contains(id))
        .collect();
    let mut orphans: Vec<ShipmentRef> = Vec::new();
    loop {
        let required: HashSet<&str> = plan
            .requirements
            .iter()
            .filter(|r| remaining.contains(&r.consumer))
            .flat_map(|r| r.providers.iter().filter(move |p| **p != r.consumer))
            .map(String::as_str)
            .collect();
        let found: Vec<&ShipmentRef> = rows
            .iter()
            .filter(|r| remaining.contains(&r.id))
            .filter(|r| r.install_reason == InstallReason::Dependency)
            .filter(|r| !required.contains(r.id.as_str()))
            .collect();
        if found.is_empty() {
            break;
        }
        for r in found {
            remaining.remove(&r.id);
            orphans.push(r.clone());
        }
    }

    Ok(RemovalPlan { removed, cascade, orphans })
}

// ---------------------------------------------------------------------------------------
// Removal: execution
// ---------------------------------------------------------------------------------------

/// What happened to a confirmed removal. Every row is accounted for: removed, failed, or not
/// attempted because an earlier one failed. Nothing is skipped silently.
#[derive(Debug, Clone, Serialize)]
pub struct RemovalOutcome {
    pub removed: Vec<String>,
    pub failed: Option<RemovalFailure>,
    pub not_attempted: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemovalFailure {
    pub id: String,
    pub error: String,
}

/// The directory directly under `staging` that holds `path`, if `path` is inside `staging`.
///
/// A registry install's row points at its Shipment root, which may be one folder down inside its
/// staging directory (archives wrap), so the directory to trash is the staging child, not the
/// row's path. A row outside `staging` is a folder the player staged from disk; that folder is
/// theirs, and removing the row leaves it alone.
fn staging_child(path: &Path, staging: &Path) -> Result<Option<PathBuf>, String> {
    let Ok(rel) = path.strip_prefix(staging) else { return Ok(None) };
    match rel.components().next() {
        Some(Component::Normal(first)) => Ok(Some(staging.join(first))),
        _ => Err(format!(
            "{} is the staging directory itself, not a Shipment inside it",
            path.display()
        )),
    }
}

/// Remove each row in order: its staging directory goes to the trash (`trash::discard`), and
/// its id is reported removed. The first failure stops the run; the rest are reported as not
/// attempted.
pub fn remove_rows(rows: &[ShipmentRef], staging: &Path, trash_into: Option<&Path>) -> RemovalOutcome {
    let mut out = RemovalOutcome { removed: Vec::new(), failed: None, not_attempted: Vec::new() };
    for (i, row) in rows.iter().enumerate() {
        let result = staging_child(Path::new(&row.path), staging).and_then(|dir| match dir {
            Some(d) => trash::discard(&d, trash_into).map(|_| ()),
            None => Ok(()),
        });
        match result {
            Ok(()) => out.removed.push(row.id.clone()),
            Err(error) => {
                out.failed = Some(RemovalFailure { id: row.id.clone(), error });
                out.not_attempted = rows[i + 1..].iter().map(|r| r.id.clone()).collect();
                break;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------

/// Compute, without removing anything, what removing `id` takes with it, for the player to
/// confirm. Runs `qm preflight` over the whole library first.
#[tauri::command]
pub async fn plan_shipment_removal(
    window: Window,
    id: String,
    rows: Vec<ShipmentRef>,
    game_path: String,
) -> Result<RemovalPlan, String> {
    let plan = preflight_rows(window, &rows, &game_path).await?;
    plan_removal(&id, &rows, &plan)
}

/// Remove exactly the rows the player confirmed, in order.
#[tauri::command(async)]
pub fn remove_shipments(rows: Vec<ShipmentRef>) -> Result<RemovalOutcome, String> {
    Ok(remove_rows(&rows, &staging_dir()?, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::load_plan::{LoadPlan, PlanItem, Producer, Requirement, RequirementStatus};
    use crate::models::origin::Origin;

    fn need(target: &str, range: Option<&str>) -> ShipmentNeed {
        ShipmentNeed::new(target, range).unwrap()
    }

    fn cand(v: &str, needs: Vec<ShipmentNeed>) -> Candidate {
        Candidate { version: Version::parse(v).unwrap(), release_version: v.into(), needs }
    }

    fn done(step: Step) -> Vec<Pick> {
        match step {
            Step::Done(p) => p,
            Step::NeedReleases(n) => panic!("still needs {n}"),
        }
    }

    /// Installing Ess pulls in lua-bridge at the highest release its range admits, never past it.
    #[test]
    fn installing_ess_pulls_in_lua_bridge_at_the_top_of_its_range() {
        let mut catalog = BTreeMap::new();
        let root = [need("lua-bridge", Some("^1.0.0"))];
        let first = resolve("ess", &root, &[], &[], &catalog).unwrap();
        assert!(matches!(first, Step::NeedReleases(ref n) if n == "lua-bridge"));

        catalog.insert(
            "lua-bridge".into(),
            vec![cand("0.5.4", vec![]), cand("1.0.0", vec![]), cand("1.2.0", vec![]), cand("2.0.0", vec![])],
        );
        let picks = done(resolve("ess", &root, &[], &[], &catalog).unwrap());
        assert_eq!(
            picks,
            vec![Pick { name: "lua-bridge".into(), release_version: "1.2.0".into(), action: Action::Install }]
        );
    }

    /// Transitive: a consumer requiring ess gets ess, and ess's own requirement gets lua-bridge.
    #[test]
    fn requirements_are_followed_transitively() {
        let mut catalog = BTreeMap::new();
        catalog.insert(
            "ess".into(),
            vec![cand("0.6.1", vec![]), cand("0.7.0", vec![need("lua-bridge", Some("^1.0.0"))])],
        );
        catalog.insert("lua-bridge".into(), vec![cand("1.0.0", vec![])]);
        let picks = done(resolve("my-mod", &[need("ess", Some(">=0.7, <1"))], &[], &[], &catalog).unwrap());
        let names: Vec<(&str, &str)> =
            picks.iter().map(|p| (p.name.as_str(), p.release_version.as_str())).collect();
        assert_eq!(names, vec![("ess", "0.7.0"), ("lua-bridge", "1.0.0")]);
    }

    /// Every range on a name counts, including ones installed rows declare.
    #[test]
    fn the_pick_satisfies_every_range_at_once() {
        let mut catalog = BTreeMap::new();
        catalog.insert(
            "lua-bridge".into(),
            vec![cand("1.0.0", vec![]), cand("1.1.0", vec![]), cand("1.4.0", vec![])],
        );
        let installed_needs = [InstalledNeed {
            declarer: "debug-overlay".into(),
            need: need("lua-bridge", Some("<1.2")),
        }];
        let picks = done(
            resolve("ess", &[need("lua-bridge", Some("^1.0.0"))], &[], &installed_needs, &catalog).unwrap(),
        );
        assert_eq!(picks[0].release_version, "1.1.0");
    }

    #[test]
    fn an_installed_dependency_that_already_is_the_pick_is_kept() {
        let mut catalog = BTreeMap::new();
        catalog.insert("lua-bridge".into(), vec![cand("1.0.0", vec![])]);
        let installed = [InstalledRow {
            id: "shipment:lb".into(),
            name: "lua-bridge".into(),
            version: Version::new(1, 0, 0),
            from_registry: true,
        }];
        let picks =
            done(resolve("ess", &[need("lua-bridge", Some("^1"))], &installed, &[], &catalog).unwrap());
        assert!(picks.is_empty(), "nothing to download: {picks:?}");
    }

    #[test]
    fn an_installed_dependency_is_updated_to_the_top_of_the_range() {
        let mut catalog = BTreeMap::new();
        catalog.insert("lua-bridge".into(), vec![cand("1.0.0", vec![]), cand("1.3.0", vec![])]);
        let installed = [InstalledRow {
            id: "shipment:lb".into(),
            name: "lua-bridge".into(),
            version: Version::new(1, 0, 0),
            from_registry: true,
        }];
        let picks =
            done(resolve("ess", &[need("lua-bridge", Some("^1"))], &installed, &[], &catalog).unwrap());
        assert_eq!(picks[0].action, Action::Update { from: "1.0.0".into() });
        assert_eq!(picks[0].release_version, "1.3.0");
    }

    /// Never go below an installed version unless the player asks.
    #[test]
    fn the_resolver_never_downgrades() {
        let mut catalog = BTreeMap::new();
        catalog.insert("lua-bridge".into(), vec![cand("1.0.0", vec![]), cand("2.0.0", vec![])]);
        let installed = [InstalledRow {
            id: "shipment:lb".into(),
            name: "lua-bridge".into(),
            version: Version::new(2, 0, 0),
            from_registry: true,
        }];
        let err = resolve("ess", &[need("lua-bridge", Some("^1"))], &installed, &[], &catalog).unwrap_err();
        assert!(err.contains("never downgrades"), "{err}");
    }

    #[test]
    fn an_unsatisfiable_range_is_an_error_naming_every_declarer() {
        let mut catalog = BTreeMap::new();
        catalog.insert("lua-bridge".into(), vec![cand("0.5.4", vec![])]);
        let err = resolve("ess", &[need("lua-bridge", Some("^1.0.0"))], &[], &[], &catalog).unwrap_err();
        assert!(err.contains("ess requires lua-bridge ^1.0.0"), "{err}");
    }

    /// A row staged from a folder is checked, never replaced.
    #[test]
    fn a_folder_row_is_kept_if_it_satisfies_and_refused_if_not() {
        let catalog = BTreeMap::new();
        let local = [InstalledRow {
            id: "shipment:lb".into(),
            name: "lua-bridge".into(),
            version: Version::new(1, 0, 0),
            from_registry: false,
        }];
        let picks = done(resolve("ess", &[need("lua-bridge", Some("^1"))], &local, &[], &catalog).unwrap());
        assert!(picks.is_empty());
        let err = resolve("ess", &[need("lua-bridge", Some("^2"))], &local, &[], &catalog).unwrap_err();
        assert!(err.contains("installed from a folder"), "{err}");
    }

    /// A range added by a pick moves an earlier pick: the top of A needs B <2, so B, first
    /// picked at 2.0.0, settles at 1.0.0.
    #[test]
    fn picks_are_revisited_until_they_settle() {
        let mut catalog = BTreeMap::new();
        catalog.insert("a".into(), vec![cand("1.0.0", vec![need("b", Some("<2"))])]);
        catalog.insert("b".into(), vec![cand("1.0.0", vec![]), cand("2.0.0", vec![])]);
        let picks = done(resolve("root", &[need("a", None), need("b", None)], &[], &[], &catalog).unwrap());
        let got: Vec<(&str, &str)> =
            picks.iter().map(|p| (p.name.as_str(), p.release_version.as_str())).collect();
        assert_eq!(got, vec![("a", "1.0.0"), ("b", "1.0.0")]);
    }

    /// my-mod needs ess (any) and lua-bridge ^1; the newest ess needs lua-bridge ^2.
    #[test]
    fn a_pick_whose_needs_contradict_another_range_is_an_error() {
        let mut catalog = BTreeMap::new();
        catalog.insert(
            "ess".into(),
            vec![
                cand("0.7.0", vec![need("lua-bridge", Some("^1"))]),
                cand("0.8.0", vec![need("lua-bridge", Some("^2"))]),
            ],
        );
        catalog.insert("lua-bridge".into(), vec![cand("1.0.0", vec![]), cand("2.0.0", vec![])]);
        let err = resolve(
            "my-mod",
            &[need("ess", None), need("lua-bridge", Some("^1"))],
            &[],
            &[],
            &catalog,
        )
        .unwrap_err();
        // ess 0.8.0 is the top of "any", and it contradicts my-mod's own lua-bridge ^1. The
        // resolver does not silently fall back to ess 0.7.0: that would be choosing an older
        // release than the range asked for on the player's behalf.
        assert!(err.contains("lua-bridge"), "{err}");
    }

    #[test]
    fn relayed_requirements_read_in_every_format_2_spelling() {
        let load = serde_json::json!({ "requires": [
            "lua-bridge",
            { "shipment": "ess", "version": "^0.7" },
            { "capability": "widescreen" }
        ]});
        let needs = relayed_shipment_needs(Some(&load), "x").unwrap();
        assert_eq!(needs.len(), 2, "the capability is not auto-installed");
        assert!(needs[0].range.is_none());
        assert_eq!(needs[1].range_text.as_deref(), Some("^0.7"));
        assert!(relayed_shipment_needs(None, "x").unwrap().is_empty());
        assert!(relayed_shipment_needs(Some(&serde_json::json!({})), "x").unwrap().is_empty());
    }

    #[test]
    fn a_retired_or_unknown_requirement_shape_is_an_error() {
        for bad in [
            serde_json::json!({ "requires": [{ "url": "https://x", "sha256": "00" }] }),
            serde_json::json!({ "requires": [{ "name": "ess", "version": "^0.7" }] }),
            serde_json::json!({ "requires": [{ "shipment": "ess", "version": "^0.7", "extra": 1 }] }),
        ] {
            assert!(relayed_shipment_needs(Some(&bad), "x").is_err(), "{bad}");
        }
    }

    // --- removal ---

    fn row(id: &str, reason: InstallReason) -> ShipmentRef {
        ShipmentRef {
            id: format!("shipment:{id}"),
            name: id.into(),
            path: format!("/staging/{id}"),
            slug: Some(id.into()),
            version: Some("1.0.0".into()),
            origin: Origin::registry(None, Some("1.0.0".into())),
            install_reason: reason,
        }
    }

    fn item(id: &str, i: usize) -> PlanItem {
        PlanItem {
            id: format!("shipment:{id}"),
            requested: i,
            resolved: Some(i),
            held_back_by: None,
            name: id.into(),
            version: "1.0.0".into(),
            manifest_format: 2,
            quartermaster_range: None,
            provides: vec![],
            plugins: vec![],
            runtime_dlls: vec![],
            placed_files: vec![],
        }
    }

    fn req(consumer: &str, kind: RequirementKind, target: &str, providers: &[&str]) -> Requirement {
        Requirement {
            consumer: format!("shipment:{consumer}"),
            index: 0,
            kind,
            target: target.into(),
            range: None,
            providers: providers.iter().map(|p| format!("shipment:{p}")).collect(),
            resolved_version: None,
            status: RequirementStatus::Satisfied,
        }
    }

    fn plan(rows: &[ShipmentRef], requirements: Vec<Requirement>) -> LoadPlan {
        LoadPlan {
            format: 1,
            producer: Producer::Preflight,
            quartermaster: "3.0.0".into(),
            ok: true,
            order: Some(rows.iter().map(|r| r.id.clone()).collect()),
            items: rows.iter().enumerate().map(|(i, r)| item(&r.name, i)).collect(),
            edges: vec![],
            requirements,
            capabilities: vec![],
            conflicts: vec![],
            supersedes: vec![],
            link_block_paths: vec![],
            findings: vec![],
        }
    }

    fn ids(v: &[ShipmentRef]) -> Vec<&str> {
        v.iter().map(|r| r.name.as_str()).collect()
    }

    /// Removing Ess orphans lua-bridge, which was installed only because Ess needed it.
    #[test]
    fn removing_ess_orphans_lua_bridge() {
        use InstallReason::*;
        let rows = vec![row("lua-bridge", Dependency), row("ess", User)];
        let p = plan(&rows, vec![req("ess", RequirementKind::Shipment, "lua-bridge", &["lua-bridge"])]);
        let r = plan_removal("shipment:ess", &rows, &p).unwrap();
        assert!(r.cascade.is_empty());
        assert_eq!(ids(&r.orphans), vec!["lua-bridge"]);
    }

    /// Removing lua-bridge takes Ess and a Shipment requiring Ess, even though both are user
    /// rows, transitively.
    #[test]
    fn removing_lua_bridge_cascades_through_ess_to_its_consumer() {
        use InstallReason::*;
        let rows = vec![
            row("lua-bridge", Dependency),
            row("ess", User),
            row("my-mod", User),
            row("unrelated", User),
        ];
        let p = plan(
            &rows,
            vec![
                req("ess", RequirementKind::Shipment, "lua-bridge", &["lua-bridge"]),
                req("my-mod", RequirementKind::Shipment, "ess", &["ess"]),
            ],
        );
        let r = plan_removal("shipment:lua-bridge", &rows, &p).unwrap();
        let cascaded: Vec<(&str, &str)> = r
            .cascade
            .iter()
            .map(|c| (c.shipment.name.as_str(), c.pulled_by.target.as_str()))
            .collect();
        assert_eq!(cascaded, vec![("ess", "lua-bridge"), ("my-mod", "ess")]);
        assert!(r.orphans.is_empty());
        let all: Vec<&str> = r.all().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(all, vec!["lua-bridge", "ess", "my-mod"], "unrelated stays");
    }

    /// Removing one of twelve skin providers takes nothing else.
    #[test]
    fn removing_one_of_many_capability_providers_takes_nothing_else() {
        use InstallReason::*;
        let mut rows: Vec<ShipmentRef> = (0..12).map(|i| row(&format!("skin{i}"), User)).collect();
        rows.push(row("x", User));
        let providers: Vec<String> = (0..12).map(|i| format!("skin{i}")).collect();
        let pr: Vec<&str> = providers.iter().map(String::as_str).collect();
        let p = plan(&rows, vec![req("x", RequirementKind::Capability, "skin", &pr)]);
        let r = plan_removal("shipment:skin3", &rows, &p).unwrap();
        assert!(r.cascade.is_empty());
        assert!(r.orphans.is_empty());
    }

    /// Removing the last provider takes the dependent.
    #[test]
    fn removing_the_last_capability_provider_takes_its_dependent() {
        use InstallReason::*;
        let rows = vec![row("provider", User), row("x", User)];
        let p = plan(&rows, vec![req("x", RequirementKind::Capability, "c", &["provider"])]);
        let r = plan_removal("shipment:provider", &rows, &p).unwrap();
        assert_eq!(r.cascade.len(), 1);
        assert_eq!(r.cascade[0].pulled_by.kind, "capability");
    }

    /// A user row nothing requires is never an orphan.
    #[test]
    fn a_user_row_is_never_an_orphan() {
        use InstallReason::*;
        let rows = vec![row("a", User), row("lonely", User)];
        let p = plan(&rows, vec![]);
        let r = plan_removal("shipment:a", &rows, &p).unwrap();
        assert!(r.orphans.is_empty());
    }

    /// The orphan pass repeats: a dependency required only by another orphan goes too.
    #[test]
    fn orphans_are_removed_to_a_fixed_point() {
        use InstallReason::*;
        let rows = vec![row("low", Dependency), row("mid", Dependency), row("top", User)];
        let p = plan(
            &rows,
            vec![
                req("top", RequirementKind::Shipment, "mid", &["mid"]),
                req("mid", RequirementKind::Shipment, "low", &["low"]),
            ],
        );
        let r = plan_removal("shipment:top", &rows, &p).unwrap();
        assert_eq!(ids(&r.orphans), vec!["mid", "low"]);
    }

    /// A dependency still required by a remaining row stays.
    #[test]
    fn a_dependency_still_required_stays() {
        use InstallReason::*;
        let rows = vec![row("lua-bridge", Dependency), row("ess", User), row("overlay", User)];
        let p = plan(
            &rows,
            vec![
                req("ess", RequirementKind::Shipment, "lua-bridge", &["lua-bridge"]),
                req("overlay", RequirementKind::Shipment, "lua-bridge", &["lua-bridge"]),
            ],
        );
        let r = plan_removal("shipment:ess", &rows, &p).unwrap();
        assert!(r.orphans.is_empty());
    }

    // --- execution ---

    fn staged_row(staging: &Path, id: &str, wrapped: bool) -> ShipmentRef {
        let dir = staging.join(format!("mercsink-{id}"));
        let root = if wrapped { dir.join("inner") } else { dir.clone() };
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("manifest.yaml"), "x").unwrap();
        ShipmentRef { path: root.to_string_lossy().into(), ..row(id, InstallReason::User) }
    }

    /// Confirm removes exactly the listed set: each staging directory goes to the trash, and a
    /// folder staged from disk is left alone.
    #[test]
    fn removal_trashes_each_staging_directory_and_leaves_player_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join("staging");
        let bin = tmp.path().join("trash");
        let a = staged_row(&staging, "a", false);
        let b = staged_row(&staging, "b", true);
        let keep = staged_row(&staging, "keep", false);
        let players = tmp.path().join("my-workshop-project");
        std::fs::create_dir_all(&players).unwrap();
        let local = ShipmentRef { path: players.to_string_lossy().into(), ..row("local", InstallReason::User) };

        let out = remove_rows(&[a, b, local], &staging, Some(&bin));
        assert_eq!(out.removed, vec!["shipment:a", "shipment:b", "shipment:local"]);
        assert!(out.failed.is_none());
        assert!(!staging.join("mercsink-a").exists());
        assert!(!staging.join("mercsink-b").exists(), "the whole staging dir, not only the root");
        assert!(Path::new(&keep.path).exists(), "an unlisted row is untouched");
        assert!(players.exists(), "the player's own folder is never trashed");
        assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 2);
    }

    /// A failure mid-way stops, naming what was removed and what was not.
    #[test]
    fn a_failed_removal_stops_and_accounts_for_every_row() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join("staging");
        let a = staged_row(&staging, "a", false);
        let gone = ShipmentRef {
            path: staging.join("mercsink-gone").to_string_lossy().into(),
            ..row("gone", InstallReason::User)
        };
        let c = staged_row(&staging, "c", false);
        let out = remove_rows(&[a, gone, c], &staging, Some(&tmp.path().join("trash")));
        assert_eq!(out.removed, vec!["shipment:a"]);
        assert_eq!(out.failed.as_ref().unwrap().id, "shipment:gone");
        assert_eq!(out.not_attempted, vec!["shipment:c"]);
        assert!(staging.join("mercsink-c").exists());
    }
}
