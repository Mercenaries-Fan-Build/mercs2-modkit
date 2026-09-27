//! mercs.ink's community incompatibility list, and the check a Shipment build runs against it.
//!
//! mercs.ink serves every community report at `GET /api/v1/incompatibilities`: "versions R1 of
//! this registered mod do not work with versions R2 of that other mod, because of X", with a
//! status. [`super::mercsink`] fetches and caches the list; this module reads it and decides what
//! it means for the Shipments about to be built.
//!
//! # Only a confirmed report blocks
//!
//! A `confirmed` report is one the subject's author confirmed on mercs.ink, and it refuses the
//! build. Every other status (`reported`, `disputed`, `resolved`) is shown as a notice and never
//! blocks: an anonymous report nobody has confirmed is information, not a verdict.
//!
//! # Matching is by public id, never by slug
//!
//! A report names its subject by mercs.ink public id, and a registry party by the same id. The
//! only rows carrying such an id are Shipments installed from mercs.ink, which record it in
//! [`Origin::id`]. A folder staged from disk has a slug but no id, and every fork of a mod shares
//! the slug, so a local row is never matched however its name reads. A report whose other party
//! is a `catalog` mod cannot match either, because no Shipment row carries a catalog origin.
//!
//! # Reading the list is strict
//!
//! Unknown keys are ignored, so mercs.ink can add fields. The closed sets (`status`, `reason`,
//! `other_source`) are enums, so a value this build does not know is an error rather than a
//! guess. Every range must parse as a semver range, the way qm reads them, and a registry other
//! party always carries one. A list that breaks any of this is refused whole: part of a list
//! could hide exactly the report that matters.
//!
//! [`Origin::id`]: crate::models::origin::Origin::id

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use super::load_plan::{nullable, LoadPlan};
use super::shipment::ShipmentRef;
use crate::models::origin::OriginSource;

// ---------------------------------------------------------------------------------------
// The list
// ---------------------------------------------------------------------------------------

/// Where a report stands on mercs.ink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Reported,
    Confirmed,
    Disputed,
    Resolved,
}

/// What goes wrong when the two mods are loaded together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    CrashOnLoad,
    Hang,
    FeatureBroken,
    SaveDamage,
    Other,
}

impl Reason {
    /// The verb phrase a notice reads: "A 1.0.0 crashes on load with B 2.0.0".
    fn label(self) -> &'static str {
        match self {
            Reason::CrashOnLoad => "crashes on load",
            Reason::Hang => "hangs",
            Reason::FeatureBroken => "breaks a feature",
            Reason::SaveDamage => "damages saves",
            Reason::Other => "has another problem",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OtherSource {
    Registry,
    Catalog,
}

#[derive(Debug, Deserialize)]
struct IndexWire {
    data: Vec<RowWire>,
    generated_at: String,
}

#[derive(Debug, Deserialize)]
struct RowWire {
    subject_id: String,
    subject_range: String,
    other_source: OtherSource,
    other_ref: String,
    /// Always sent; `null` only for a catalog party. Read through `nullable` so a missing key is
    /// an error rather than a silent `None`.
    #[serde(deserialize_with = "nullable")]
    other_range: Option<String>,
    reason: Reason,
    status: Status,
    reports: u32,
    updated_on: String,
}

/// The other party of a report.
#[derive(Debug, Clone)]
pub enum OtherParty {
    /// A mod registered on mercs.ink, by public id, with the range it is affected in.
    Registry { id: String, range: VersionReq },
    /// A catalog mod. Its ref and range were read and checked, but no Shipment row carries a
    /// catalog origin, so nothing is kept to match against.
    Catalog,
}

/// One report, read and checked.
#[derive(Debug, Clone)]
pub struct Incompatibility {
    pub subject_id: String,
    pub subject_range: VersionReq,
    pub other: OtherParty,
    pub reason: Reason,
    pub status: Status,
    pub reports: u32,
    pub updated_on: String,
}

/// The whole list as mercs.ink served it.
#[derive(Debug, Clone)]
pub struct IncompatibilityIndex {
    pub rows: Vec<Incompatibility>,
    /// When mercs.ink rendered the list. Its response may be cached for a while, so this is not
    /// the list's age on this machine; that is the time Modkit last fetched it.
    pub generated_at: String,
}

fn parse_range(text: &str, field: &str, row: usize) -> Result<VersionReq, String> {
    VersionReq::parse(text).map_err(|e| {
        format!(
            "Row {row} of mercs.ink's incompatibility list has {field} \"{text}\", which is not a \
             semver range: {e}"
        )
    })
}

/// Read the body of `GET /api/v1/incompatibilities`. Any row that breaks the contract refuses
/// the whole list.
pub fn parse_index(body: &str) -> Result<IncompatibilityIndex, String> {
    let wire: IndexWire = serde_json::from_str(body)
        .map_err(|e| format!("mercs.ink's incompatibility list could not be read: {e}"))?;
    let rows = wire
        .data
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let n = i + 1;
            let subject_range = parse_range(&r.subject_range, "subject_range", n)?;
            let other_range = r
                .other_range
                .as_deref()
                .map(|t| parse_range(t, "other_range", n))
                .transpose()?;
            let other = match (r.other_source, other_range) {
                (OtherSource::Registry, Some(range)) => OtherParty::Registry { id: r.other_ref, range },
                (OtherSource::Registry, None) => {
                    return Err(format!(
                        "Row {n} of mercs.ink's incompatibility list names the registered mod {} \
                         with no other_range. A registered mod has versions, so mercs.ink always \
                         sends one for it.",
                        r.other_ref
                    ))
                }
                (OtherSource::Catalog, _) => OtherParty::Catalog,
            };
            Ok(Incompatibility {
                subject_id: r.subject_id,
                subject_range,
                other,
                reason: r.reason,
                status: r.status,
                reports: r.reports,
                updated_on: r.updated_on,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(IncompatibilityIndex { rows, generated_at: wire.generated_at })
}

// ---------------------------------------------------------------------------------------
// Checking a set of Shipments
// ---------------------------------------------------------------------------------------

/// Which version of each row a check compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionBasis {
    /// The version recorded when the row was installed (`origin.version`). Checked before
    /// preflight, so a refusal from preflight can still report it.
    Recorded,
    /// The version qm read from the Shipment's manifest during preflight.
    Manifest,
}

/// One report that applies to the set: a subject row and a different other row, each inside its
/// range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    report: usize,
    subject: usize,
    other: usize,
    subject_version: String,
    other_version: String,
    recorded: bool,
    manifest: bool,
}

impl Found {
    fn same_match(&self, other: &Found) -> bool {
        self.report == other.report
            && self.subject == other.subject
            && self.other == other.other
            && self.subject_version == other.subject_version
            && self.other_version == other.other_version
    }
}

/// The version each row was installed at, in row order.
pub fn recorded_versions(rows: &[ShipmentRef]) -> Vec<Option<&str>> {
    rows.iter().map(|r| r.origin.version.as_deref()).collect()
}

/// The version qm read for each row, in row order. `plan` answers a request listing `rows` in
/// order, so item `i` is row `i`; anything else is an error.
pub fn manifest_versions<'a>(
    plan: &'a LoadPlan,
    rows: &[ShipmentRef],
) -> Result<Vec<Option<&'a str>>, String> {
    if plan.items.len() != rows.len() {
        return Err(format!(
            "qm preflight reported {} items for {} Shipments, so their versions cannot be checked \
             against mercs.ink's incompatibility list",
            plan.items.len(),
            rows.len()
        ));
    }
    plan.items
        .iter()
        .zip(rows)
        .map(|(item, row)| {
            if item.id == row.id {
                Ok(Some(item.version.as_str()))
            } else {
                Err(format!(
                    "qm preflight's item {} does not match the Shipment {} in the same place",
                    item.id, row.id
                ))
            }
        })
        .collect()
}

/// Rows that are the registered mod `id` at a version inside `range`, with that version.
fn rows_in_range(
    rows: &[ShipmentRef],
    versions: &[Option<&str>],
    id: &str,
    range: &VersionReq,
    basis: VersionBasis,
) -> Result<Vec<(usize, String)>, String> {
    let mut out = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        if row.origin.source != OriginSource::Registry || row.origin.id.as_deref() != Some(id) {
            continue;
        }
        let what = match basis {
            VersionBasis::Recorded => "recorded version",
            VersionBasis::Manifest => "manifest version qm read",
        };
        let text = versions[i].ok_or_else(|| {
            format!(
                "{} has no {what}, so it cannot be checked against mercs.ink's incompatibility list",
                row.name
            )
        })?;
        let version = Version::parse(text).map_err(|e| {
            format!(
                "{}'s {what} \"{text}\" is not semver, so it cannot be checked against mercs.ink's \
                 incompatibility list: {e}",
                row.name
            )
        })?;
        if range.matches(&version) {
            out.push((i, text.to_string()));
        }
    }
    Ok(out)
}

/// Every report in `index` that applies to `rows` at `versions` (one per row, in row order).
pub fn evaluate(
    index: &IncompatibilityIndex,
    rows: &[ShipmentRef],
    versions: &[Option<&str>],
    basis: VersionBasis,
) -> Result<Vec<Found>, String> {
    if versions.len() != rows.len() {
        return Err(format!(
            "{} versions were given for {} Shipments; the incompatibility check needs one per row",
            versions.len(),
            rows.len()
        ));
    }
    let mut found = Vec::new();
    for (report, inc) in index.rows.iter().enumerate() {
        let OtherParty::Registry { id: other_id, range: other_range } = &inc.other else {
            continue;
        };
        let subjects = rows_in_range(rows, versions, &inc.subject_id, &inc.subject_range, basis)?;
        if subjects.is_empty() {
            continue;
        }
        let others = rows_in_range(rows, versions, other_id, other_range, basis)?;
        for (subject, subject_version) in &subjects {
            for (other, other_version) in &others {
                if subject == other {
                    continue;
                }
                found.push(Found {
                    report,
                    subject: *subject,
                    other: *other,
                    subject_version: subject_version.clone(),
                    other_version: other_version.clone(),
                    recorded: basis == VersionBasis::Recorded,
                    manifest: basis == VersionBasis::Manifest,
                });
            }
        }
    }
    Ok(found)
}

/// One list of matches from the check before preflight and the one after. A match both found at
/// the same versions is listed once, marked as found by both.
pub fn merge(before: Vec<Found>, after: Vec<Found>) -> Vec<Found> {
    let mut out = before;
    for f in after {
        match out.iter_mut().find(|o| o.same_match(&f)) {
            Some(o) => o.manifest = true,
            None => out.push(f),
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// What the player reads
// ---------------------------------------------------------------------------------------

fn basis_phrase(f: &Found) -> &'static str {
    match (f.recorded, f.manifest) {
        (true, true) => "the versions recorded at install and the versions qm read from the manifests",
        (true, false) => "the versions recorded when they were installed",
        _ => "the versions qm read from the manifests",
    }
}

fn plural(n: u64, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

fn headline(inc: &Incompatibility, rows: &[ShipmentRef], f: &Found) -> String {
    format!(
        "{} {} {} with {} {}",
        rows[f.subject].name,
        f.subject_version,
        inc.reason.label(),
        rows[f.other].name,
        f.other_version
    )
}

fn confirmed_lines(index: &IncompatibilityIndex, rows: &[ShipmentRef], found: &[Found]) -> Vec<String> {
    found
        .iter()
        .filter(|f| index.rows[f.report].status == Status::Confirmed)
        .map(|f| {
            let inc = &index.rows[f.report];
            format!(
                "{} (confirmed on mercs.ink, updated {}; checked with {})",
                headline(inc, rows, f),
                inc.updated_on,
                basis_phrase(f)
            )
        })
        .collect()
}

fn incompatibilities(n: usize) -> &'static str {
    if n == 1 {
        "a confirmed incompatibility"
    } else {
        "confirmed incompatibilities"
    }
}

/// The refusal when qm preflight itself refused: preflight's own text, with every confirmed
/// report the check before preflight found added to it.
pub fn with_preflight_refusal(
    preflight: String,
    index: &IncompatibilityIndex,
    rows: &[ShipmentRef],
    before: &[Found],
) -> String {
    let lines = confirmed_lines(index, rows, before);
    if lines.is_empty() {
        return preflight;
    }
    format!(
        "{preflight}\n\nmercs.ink also lists {} in this set of Shipments:\n{}",
        incompatibilities(lines.len()),
        lines.join("\n")
    )
}

/// After preflight passed: refuse when any confirmed report applies, and otherwise return the
/// notices for every other report that does.
pub fn gate(
    index: &IncompatibilityIndex,
    rows: &[ShipmentRef],
    found: &[Found],
) -> Result<Vec<Notice>, String> {
    let lines = confirmed_lines(index, rows, found);
    if !lines.is_empty() {
        return Err(format!(
            "mercs.ink lists {} in this set of Shipments, so nothing was built:\n{}",
            incompatibilities(lines.len()),
            lines.join("\n")
        ));
    }
    Ok(found.iter().map(|f| notice(index, rows, f)).collect())
}

/// The subject of a notice: the row that is the reported mod.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct NoticeSubject {
    pub name: String,
    pub version: String,
    pub id: String,
}

/// The other party of a notice: the row it is reported against.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct NoticeOther {
    pub name: String,
    pub version: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

/// An unconfirmed report that applies to the build. Shown, never blocking.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Notice {
    pub status: Status,
    pub reason: Reason,
    pub message: String,
    pub subject: NoticeSubject,
    pub other: NoticeOther,
    pub reports: u32,
    pub updated_on: String,
}

fn notice(index: &IncompatibilityIndex, rows: &[ShipmentRef], f: &Found) -> Notice {
    let inc = &index.rows[f.report];
    let subject = &rows[f.subject];
    let other = &rows[f.other];
    let reports = plural(u64::from(inc.reports), "player report");
    let standing = match inc.status {
        Status::Reported => format!("{reports} on mercs.ink, not confirmed by its author"),
        Status::Disputed => format!("{reports} on mercs.ink; the author of {} disputes it", subject.name),
        Status::Resolved => format!(
            "the author of {} marked it resolved, and {} is still inside the affected range",
            subject.name, f.subject_version
        ),
        // `gate` refuses before any notice is made for a confirmed report.
        Status::Confirmed => "confirmed on mercs.ink".to_string(),
    };
    Notice {
        status: inc.status,
        reason: inc.reason,
        message: format!(
            "{}: {standing} (updated {}; checked with {}).",
            headline(inc, rows, f),
            inc.updated_on,
            basis_phrase(f)
        ),
        subject: NoticeSubject {
            name: subject.name.clone(),
            version: f.subject_version.clone(),
            id: inc.subject_id.clone(),
        },
        other: NoticeOther {
            name: other.name.clone(),
            version: f.other_version.clone(),
            reference: other.origin.id.clone().unwrap_or_default(),
        },
        reports: inc.reports,
        updated_on: inc.updated_on.clone(),
    }
}

// ---------------------------------------------------------------------------------------
// How fresh the list is
// ---------------------------------------------------------------------------------------

/// How long ago `then` was, as of `now`, both in seconds since the epoch. Reads after
/// "downloaded".
pub fn describe_age(now: u64, then: u64) -> String {
    let Some(secs) = now.checked_sub(then) else {
        return "at a time later than this computer's clock now reads, so its age is unknown"
            .to_string();
    };
    match secs {
        0..=59 => "just now".to_string(),
        60..=3_599 => format!("{} ago", plural(secs / 60, "minute")),
        3_600..=86_399 => format!("{} ago", plural(secs / 3_600, "hour")),
        _ => format!("{} ago", plural(secs / 86_400, "day")),
    }
}

/// Which list a build was checked against.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ListState {
    /// mercs.ink answered, with a new list or by confirming the cached one.
    Current { fetched_at: u64 },
    /// mercs.ink could not be reached, so the list Modkit last downloaded was used.
    Cached { fetched_at: u64, generated_at: String, reason: String, message: String },
    /// mercs.ink could not be reached and Modkit has never downloaded the list, so the build
    /// was not checked against it.
    NeverFetched { reason: String, message: String },
}

impl ListState {
    /// `reason` says why mercs.ink could not be reached.
    pub fn cached(fetched_at: u64, generated_at: String, reason: String, now: u64) -> Self {
        let message = format!(
            "Couldn't check mercs.ink for incompatibility reports ({reason}). This build was \
             checked against the list Modkit downloaded {}.",
            describe_age(now, fetched_at)
        );
        ListState::Cached { fetched_at, generated_at, reason, message }
    }

    /// `reason` says why mercs.ink could not be reached.
    pub fn never_fetched(reason: String) -> Self {
        let message = format!(
            "Couldn't check mercs.ink for incompatibility reports ({reason}), and Modkit has never \
             downloaded the list, so this build was not checked against it."
        );
        ListState::NeverFetched { reason, message }
    }
}

/// The list a build is checked against, and where it came from. `index` is `None` only when
/// the list has never been downloaded.
#[derive(Debug)]
pub struct LoadedList {
    pub state: ListState,
    pub index: Option<IncompatibilityIndex>,
}

/// What a build reports about the list: which list it used and the reports that apply without
/// blocking.
#[derive(Debug, Clone, Serialize)]
pub struct IncompatibilityCheck {
    pub list: ListState,
    pub notices: Vec<Notice>,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::commands::load_plan::tests::{chain_request, CHAIN_PLAN};
    use crate::commands::load_plan::Producer;
    use crate::commands::shipment::InstallReason;
    use crate::models::origin::Origin;

    /// The shape `IncompatibilityResource` renders, with every status and a catalog party.
    pub(crate) const EXAMPLE: &str = r#"{
      "data": [
        { "subject_id": "vehicle-pack-111", "subject_range": "^1.0.0",
          "other_source": "registry", "other_ref": "ess-222", "other_range": ">=0.7.0, <0.8.0",
          "reason": "crash_on_load", "status": "confirmed", "reports": 4, "updated_on": "2026-09-01" },
        { "subject_id": "vehicle-pack-111", "subject_range": "^1.0.0",
          "other_source": "catalog", "other_ref": "https://github.com/elishacloud/dxwrapper#dxwrapper",
          "other_range": null, "reason": "hang", "status": "reported", "reports": 1, "updated_on": "2026-09-02" }
      ],
      "generated_at": "2026-09-25T10:00:00+00:00"
    }"#;

    fn row(name: &str, source: OriginSource, id: Option<&str>, version: &str) -> ShipmentRef {
        ShipmentRef {
            id: format!("shipment:{name}"),
            name: name.into(),
            path: format!("/staging/{name}"),
            slug: Some(name.into()),
            version: Some(version.into()),
            origin: Origin { source, id: id.map(str::to_string), version: Some(version.into()) },
            install_reason: InstallReason::User,
        }
    }

    fn registry(name: &str, id: &str, version: &str) -> ShipmentRef {
        row(name, OriginSource::Registry, Some(id), version)
    }

    fn one_report(status: &str, other_source: &str, other_ref: &str, other_range: &str) -> IncompatibilityIndex {
        parse_index(&format!(
            r#"{{"data":[{{"subject_id":"a-1","subject_range":"^1.0.0","other_source":"{other_source}",
                "other_ref":"{other_ref}","other_range":{other_range},"reason":"hang","status":"{status}",
                "reports":3,"updated_on":"2026-09-10"}}],"generated_at":"2026-09-25T00:00:00+00:00"}}"#
        ))
        .unwrap()
    }

    fn check(index: &IncompatibilityIndex, rows: &[ShipmentRef]) -> Vec<Found> {
        evaluate(index, rows, &recorded_versions(rows), VersionBasis::Recorded).unwrap()
    }

    // --- parsing ---

    #[test]
    fn the_example_body_parses() {
        let index = parse_index(EXAMPLE).unwrap();
        assert_eq!(index.rows.len(), 2);
        assert_eq!(index.generated_at, "2026-09-25T10:00:00+00:00");
        let first = &index.rows[0];
        assert_eq!(first.subject_id, "vehicle-pack-111");
        assert_eq!(first.status, Status::Confirmed);
        assert_eq!(first.reason, Reason::CrashOnLoad);
        assert_eq!(first.reports, 4);
        assert!(matches!(&first.other, OtherParty::Registry { id, .. } if id == "ess-222"));
        assert!(matches!(index.rows[1].other, OtherParty::Catalog));
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let body = EXAMPLE
            .replace(r#""reports": 4,"#, r#""reports": 4, "brand_new": {"x": 1},"#)
            .replace(r#""generated_at""#, r#""links": null, "generated_at""#);
        assert_eq!(parse_index(&body).unwrap().rows.len(), 2);
    }

    #[test]
    fn an_unknown_status_reason_or_source_is_refused() {
        for (from, to) in [
            (r#""status": "confirmed""#, r#""status": "maybe""#),
            (r#""reason": "crash_on_load""#, r#""reason": "slow""#),
            (r#""other_source": "registry""#, r#""other_source": "local""#),
        ] {
            let body = EXAMPLE.replace(from, to);
            assert_ne!(body, EXAMPLE, "the fixture must contain {from}");
            assert!(parse_index(&body).is_err(), "{to} must be refused");
        }
    }

    #[test]
    fn a_range_that_is_not_semver_is_refused() {
        let err = parse_index(&EXAMPLE.replace(r#""subject_range": "^1.0.0""#, r#""subject_range": "one-ish""#))
            .unwrap_err();
        assert!(err.contains("Row 1") && err.contains("subject_range") && err.contains("one-ish"), "{err}");
        let err = parse_index(&EXAMPLE.replace(">=0.7.0, <0.8.0", "latest")).unwrap_err();
        assert!(err.contains("other_range"), "{err}");
    }

    #[test]
    fn a_registry_party_without_a_range_is_a_contract_error() {
        let err = parse_index(&EXAMPLE.replace(r#""other_range": ">=0.7.0, <0.8.0""#, r#""other_range": null"#))
            .unwrap_err();
        assert!(err.contains("ess-222") && err.contains("other_range"), "{err}");
    }

    #[test]
    fn a_missing_key_is_refused() {
        let body = EXAMPLE.replace(r#""other_range": null, "#, "");
        assert_ne!(body, EXAMPLE);
        assert!(parse_index(&body).is_err(), "other_range must be present, even when null");
        assert!(parse_index(r#"{"data":[]}"#).is_err(), "generated_at is required");
    }

    // --- evaluating ---

    #[test]
    fn a_confirmed_report_with_both_parties_in_range_blocks() {
        let index = one_report("confirmed", "registry", "b-2", r#""^2.0.0""#);
        let rows = vec![registry("A", "a-1", "1.2.0"), registry("B", "b-2", "2.0.1")];
        let found = check(&index, &rows);
        assert_eq!(found.len(), 1);
        let err = gate(&index, &rows, &found).unwrap_err();
        assert_eq!(
            err,
            "mercs.ink lists a confirmed incompatibility in this set of Shipments, so nothing was \
             built:\nA 1.2.0 hangs with B 2.0.1 (confirmed on mercs.ink, updated 2026-09-10; checked \
             with the versions recorded when they were installed)"
        );
    }

    #[test]
    fn a_party_out_of_range_or_absent_does_not_match() {
        let index = one_report("confirmed", "registry", "b-2", r#""^2.0.0""#);
        let subject_out = vec![registry("A", "a-1", "2.0.0"), registry("B", "b-2", "2.0.0")];
        assert!(check(&index, &subject_out).is_empty());
        let other_out = vec![registry("A", "a-1", "1.0.0"), registry("B", "b-2", "3.0.0")];
        assert!(check(&index, &other_out).is_empty());
        let other_absent = vec![registry("A", "a-1", "1.0.0")];
        assert!(check(&index, &other_absent).is_empty());
    }

    #[test]
    fn other_statuses_are_notices_only() {
        let rows = vec![registry("A", "a-1", "1.0.0"), registry("B", "b-2", "2.0.0")];
        for (status, standing) in [
            ("reported", "3 player reports on mercs.ink, not confirmed by its author"),
            ("disputed", "3 player reports on mercs.ink; the author of A disputes it"),
            ("resolved", "the author of A marked it resolved, and 1.0.0 is still inside the affected range"),
        ] {
            let index = one_report(status, "registry", "b-2", r#""^2.0.0""#);
            let notices = gate(&index, &rows, &check(&index, &rows)).unwrap();
            assert_eq!(notices.len(), 1, "{status}");
            let n = &notices[0];
            assert_eq!(
                n.message,
                format!(
                    "A 1.0.0 hangs with B 2.0.0: {standing} (updated 2026-09-10; checked with the \
                     versions recorded when they were installed)."
                )
            );
            assert_eq!(n.subject, NoticeSubject { name: "A".into(), version: "1.0.0".into(), id: "a-1".into() });
            assert_eq!(n.other, NoticeOther { name: "B".into(), version: "2.0.0".into(), reference: "b-2".into() });
            assert_eq!(n.reports, 3);
            assert_eq!(n.updated_on, "2026-09-10");
        }
    }

    #[test]
    fn local_rows_and_rows_without_an_id_never_match() {
        let index = one_report("confirmed", "registry", "b-2", r#""^2.0.0""#);
        // The subject's slug staged from disk: same name, no public id.
        let local = vec![row("a", OriginSource::Local, None, "1.0.0"), registry("B", "b-2", "2.0.0")];
        assert!(check(&index, &local).is_empty());
        let no_id = vec![row("A", OriginSource::Registry, None, "1.0.0"), registry("B", "b-2", "2.0.0")];
        assert!(check(&index, &no_id).is_empty());
        let imported = vec![row("A", OriginSource::Imported, Some("a-1"), "1.0.0"), registry("B", "b-2", "2.0.0")];
        assert!(check(&index, &imported).is_empty(), "the id only counts on a registry row");
    }

    #[test]
    fn a_report_naming_the_subject_as_its_own_other_party_never_matches_one_row() {
        let index = one_report("confirmed", "registry", "a-1", r#""^1.0.0""#);
        assert!(check(&index, &[registry("A", "a-1", "1.0.0")]).is_empty());
    }

    #[test]
    fn a_catalog_other_party_is_ignored() {
        let index = parse_index(EXAMPLE).unwrap();
        let rows = vec![registry("Vehicle Pack", "vehicle-pack-111", "1.0.0")];
        assert!(check(&index, &rows).is_empty());
    }

    #[test]
    fn a_caret_range_does_not_match_a_prerelease() {
        let index = one_report("confirmed", "registry", "b-2", r#""^2.0.0""#);
        let rows = vec![registry("A", "a-1", "1.1.0-beta.1"), registry("B", "b-2", "2.0.0")];
        assert!(check(&index, &rows).is_empty());
    }

    /// Recorded versions say ess 0.6.0; qm reads 0.7.0. Only the check that uses qm's versions
    /// finds the report, and it says so.
    #[test]
    fn the_before_check_uses_recorded_versions_and_the_after_check_uses_the_plan() {
        let plan = LoadPlan::parse(CHAIN_PLAN, &chain_request(), Producer::Preflight).unwrap();
        let rows = vec![
            registry("ess", "ess-1", "0.6.0"),
            registry("lua-bridge", "lua-bridge-2", "1.0.0"),
            registry("my-mod", "my-mod-3", "1.0.0"),
        ];
        let index = parse_index(
            r#"{"data":[{"subject_id":"ess-1","subject_range":"^0.7.0","other_source":"registry",
                "other_ref":"lua-bridge-2","other_range":"^1.0.0","reason":"hang","status":"confirmed",
                "reports":0,"updated_on":"2026-09-10"}],"generated_at":"x"}"#,
        )
        .unwrap();

        let before = evaluate(&index, &rows, &recorded_versions(&rows), VersionBasis::Recorded).unwrap();
        assert!(before.is_empty(), "the recorded 0.6.0 is outside ^0.7.0");
        let after = evaluate(&index, &rows, &manifest_versions(&plan, &rows).unwrap(), VersionBasis::Manifest)
            .unwrap();
        assert_eq!(after.len(), 1);
        let err = gate(&index, &rows, &merge(before, after)).unwrap_err();
        assert!(err.contains("ess 0.7.0 hangs with lua-bridge 1.0.0"), "{err}");
        assert!(err.contains("checked with the versions qm read from the manifests"), "{err}");
    }

    #[test]
    fn a_non_semver_plan_version_is_an_error() {
        let text = CHAIN_PLAN.replace(r#""version": "0.7.0""#, r#""version": "seven""#);
        assert_ne!(text, CHAIN_PLAN);
        let plan = LoadPlan::parse(&text, &chain_request(), Producer::Preflight).unwrap();
        let rows = vec![
            registry("ess", "ess-1", "0.7.0"),
            registry("lua-bridge", "lua-bridge-2", "1.0.0"),
            registry("my-mod", "my-mod-3", "1.0.0"),
        ];
        let index = one_report("confirmed", "registry", "lua-bridge-2", r#""^1.0.0""#);
        let index = IncompatibilityIndex {
            rows: vec![Incompatibility { subject_id: "ess-1".into(), ..index.rows[0].clone() }],
            ..index
        };
        let err = evaluate(&index, &rows, &manifest_versions(&plan, &rows).unwrap(), VersionBasis::Manifest)
            .unwrap_err();
        assert!(err.contains("ess") && err.contains("\"seven\"") && err.contains("not semver"), "{err}");
    }

    #[test]
    fn a_plan_answering_other_rows_is_an_error() {
        let plan = LoadPlan::parse(CHAIN_PLAN, &chain_request(), Producer::Preflight).unwrap();
        let rows = vec![registry("ess", "ess-1", "0.7.0")];
        assert!(manifest_versions(&plan, &rows).is_err());
    }

    // --- text ---

    #[test]
    fn a_match_both_checks_found_is_listed_once() {
        let index = one_report("confirmed", "registry", "b-2", r#""^2.0.0""#);
        let rows = vec![registry("A", "a-1", "1.0.0"), registry("B", "b-2", "2.0.0")];
        let versions = recorded_versions(&rows);
        let before = evaluate(&index, &rows, &versions, VersionBasis::Recorded).unwrap();
        let after = evaluate(&index, &rows, &versions, VersionBasis::Manifest).unwrap();
        let merged = merge(before, after);
        assert_eq!(merged.len(), 1);
        let err = gate(&index, &rows, &merged).unwrap_err();
        assert!(
            err.ends_with(
                "(confirmed on mercs.ink, updated 2026-09-10; checked with the versions recorded at \
                 install and the versions qm read from the manifests)"
            ),
            "{err}"
        );
    }

    #[test]
    fn a_preflight_refusal_carries_the_confirmed_reports_found_before_it() {
        let index = one_report("confirmed", "registry", "b-2", r#""^2.0.0""#);
        let rows = vec![registry("A", "a-1", "1.0.0"), registry("B", "b-2", "2.0.0")];
        let before = check(&index, &rows);
        let preflight = "qm preflight refused this set of Shipments, so nothing was built:\nM0001 error: x".to_string();
        assert_eq!(
            with_preflight_refusal(preflight.clone(), &index, &rows, &before),
            format!(
                "{preflight}\n\nmercs.ink also lists a confirmed incompatibility in this set of \
                 Shipments:\nA 1.0.0 hangs with B 2.0.0 (confirmed on mercs.ink, updated 2026-09-10; \
                 checked with the versions recorded when they were installed)"
            )
        );
        // Nothing confirmed: preflight's refusal is unchanged.
        let reported = one_report("reported", "registry", "b-2", r#""^2.0.0""#);
        assert_eq!(
            with_preflight_refusal(preflight.clone(), &reported, &rows, &check(&reported, &rows)),
            preflight
        );
    }

    #[test]
    fn ages_read_in_the_largest_whole_unit() {
        let t = 1_000_000;
        assert_eq!(describe_age(t, t), "just now");
        assert_eq!(describe_age(t + 59, t), "just now");
        assert_eq!(describe_age(t + 60, t), "1 minute ago");
        assert_eq!(describe_age(t + 3_599, t), "59 minutes ago");
        assert_eq!(describe_age(t + 3_600, t), "1 hour ago");
        assert_eq!(describe_age(t + 86_399, t), "23 hours ago");
        assert_eq!(describe_age(t + 86_400, t), "1 day ago");
        assert_eq!(describe_age(t + 10 * 86_400, t), "10 days ago");
    }

    #[test]
    fn a_clock_behind_the_download_says_so_instead_of_an_age() {
        assert_eq!(
            describe_age(100, 101),
            "at a time later than this computer's clock now reads, so its age is unknown"
        );
    }

    #[test]
    fn list_states_serialize_tagged_with_their_text() {
        let cached = ListState::cached(1_000, "g".into(), "it answered HTTP 503".into(), 1_000 + 7_200);
        let v = serde_json::to_value(&cached).unwrap();
        assert_eq!(v["state"], "cached");
        assert_eq!(v["fetched_at"], 1_000);
        assert_eq!(
            v["message"],
            "Couldn't check mercs.ink for incompatibility reports (it answered HTTP 503). This build \
             was checked against the list Modkit downloaded 2 hours ago."
        );
        let never = serde_json::to_value(ListState::never_fetched("could not connect".into())).unwrap();
        assert_eq!(never["state"], "never_fetched");
        assert_eq!(serde_json::to_value(ListState::Current { fetched_at: 5 }).unwrap()["state"], "current");
    }
}
