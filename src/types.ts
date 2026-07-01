// TypeScript mirrors of the Rust models (serde field names).

export interface ManifestAsset {
  path: string;
  name: string;
  type: string; // "auto" or explicit type
  target_patch: string; // "auto" or named group
}

export interface ManifestRequirements {
  game_version: string | null;
}

export interface Manifest {
  name: string;
  version: string;
  author: string | null;
  description: string | null;
  requirements: ManifestRequirements;
  dependencies: string[];
  assets: ManifestAsset[];
}

export interface DetectedAsset {
  path: string;
  abs_path: string;
  name: string;
  asset_hash: number;
  detected_type: string;
  target_patch: string;
}

export interface LoadedMod {
  id: string;
  root: string;
  manifest: Manifest;
  assets: DetectedAsset[];
}

export interface AssetConflict {
  asset_hash: number;
  asset_name: string | null;
  mods: string[];
}

export interface ConflictGraph {
  conflicts: AssetConflict[];
}

export interface ValidationError {
  field: string;
  message: string;
}

export interface BuiltWad {
  path: string;
  patch_group: string;
  block_count: number;
  byte_size: number;
}

export interface BuildResult {
  outputs: BuiltWad[];
}

export interface ValidationResult {
  ok: boolean;
  exit_code: number | null;
  stdout: string;
  stderr: string;
}

/** How the user chose to resolve one conflict (keyed by asset_hash). */
export type Resolution =
  | { kind: "priority"; modId: string }
  | { kind: "exclude"; modId: string };

export interface GameInfo {
  root: string;
  exe_path: string;
  exe_size: number;
  version: string; // "v1.0" | "v1.1" | "unknown"
  variant: string; // "unsigned" | "ea-signed" | "patched" | "cracked" | "unknown"
  has_pmc_bb: boolean;
  asi_loader_proxy: string | null; // e.g. "pmc_bb.dll", or null if none
  data_dir: string | null;
  deployed_patches: string[];
  deployed_asi: DeployedAsi[];
  log_path: string | null;
}

/** A .asi plugin found already deployed in the game install. */
export interface DeployedAsi {
  name: string;
  rel_path: string;
  abs_path: string;
  size: number;
  known: string | null;
}

/** A repository source entry (mirrors Rust's RepoSource). */
export interface RepoSource {
  name: string;
  description: string;
  repository: string;
  /** Branch to read repository.json from. Omit to fall back to main/master. */
  branch?: string;
}

/** One enableable mod, expanded from a source repo's index. */
export interface CatalogMod {
  repository: string; // source repo URL
  repo_name: string; // display name of the source repository
  slug: string; // mod id, unique within its repository
  name: string;
  description: string;
  kind: string; // "asi" | "wad" (informational)
  assets: string[]; // release asset filenames this mod deploys
  version: string | null;
  incompatible: string[]; // "repo-url#slug" refs that must not be enabled alongside this
}

export interface Catalog {
  mods: CatalogMod[];
  source: string; // "remote" | "bundled"
}

export interface InstallResult {
  mod_root: string;
  kind: string; // "wad" | "asi"
  version: string;
  asi_files: string[];
  staged_files: number;
}

/** An installed ASI-plugin mod staged on disk, ready to deploy. */
export interface AsiMod {
  id: string;
  name: string;
  description: string;
  version: string;
  modRoot: string;
  asiFiles: string[];
}

export interface DeployResult {
  target_dir: string;
  deployed: string[];
  backed_up: string[];
}

export interface TrashResult {
  trashed: string[];
  missing: string[];
  trash_dir: string | null;
}

export interface ReleaseInfo {
  tag: string;
  name: string;
  url: string;
  body: string;
}

export interface ModkitUpdate {
  current: string;
  latest: string;
  url: string;
  available: boolean;
}

/**
 * Release-update status for one of modkit's core components (the pmc_bb.dll ASI
 * loader, the apply_crack SecuROM-bypass tool). `current` is the version modkit
 * last installed, or null if unknown (installed out-of-band / before tracking).
 */
export interface ComponentUpdate {
  /** Human label, e.g. "pmc_bb.dll (ASI loader)". */
  name: string;
  current: string | null;
  latest: string;
  url: string;
  available: boolean;
}

export interface InstallDllResult {
  path: string;
  version: string;
}

export interface CrackResult {
  ok: boolean;
  output_path: string;
  stdout: string;
  stderr: string;
  tool_version: string; // apply_crack release tag that was downloaded & run
}

/** Whether the 32-bit Microsoft Visual C++ 2008 runtime is installed. */
export interface VcRedistStatus {
  applicable: boolean; // false on non-Windows hosts (handled by the Proton prefix)
  installed: boolean;
  detail: string;
}

export interface InstallVcRedistResult {
  installed: boolean;
  already_present: boolean;
  message: string;
}

/** A vanilla file that exists but no longer matches its manifest fingerprint. */
export interface FileDiff {
  path: string;
  expected_size: number;
  actual_size: number;
  expected_hash: string;
  actual_hash: string;
}

/** Identification of one on-disk executable against the catalog. */
export interface ExeReport {
  file: string;
  size: number;
  hash: string;
  identifiedAs: string | null; // catalog description when the hash matched
  notes: string[]; // unrecognized hint, missing sidecar DLL, modding caveat
}

/** Block-level diff for one WAD whose whole-file hash didn't match. */
export interface WadDiff {
  wad: string;
  modified: string[]; // vanilla blocks present but changed
  missing: string[]; // vanilla blocks absent
  added: string[]; // blocks with no vanilla counterpart (added content)
  affectedAssets: number; // catalogued assets carried by the changed/missing blocks
}

/** Result of verifying the install against a known-good manifest. */
export interface VerifyReport {
  ok: number;
  missing: string[]; // shared files absent on disk (critical)
  corrupt: FileDiff[]; // present but changed/damaged
  extra: string[]; // on disk, not in the manifest (mods/saves) — informational
  ignored: number; // excluded files skipped (exe, caches, config, mods)
  exes: ExeReport[]; // identification of the main + cracked executables
  wadDetails: WadDiff[]; // per-WAD block breakdown for mismatched WADs
  manifestSource: string;
}

export interface GenerateManifestResult {
  path: string;
  fileCount: number;
  blockCount: number;
  totalBytes: number;
}

/** Result of bundling logs, a mod inventory, versions, and an integrity check
 *  into a dated debug `.zip`. */
export interface DebugZipResult {
  path: string;
  bytes: number;
  logCount: number;
  integrityOk: boolean;
  notes: string[];
}

/** `verify-progress` / `manifest-progress` event payload. */
export interface HashProgress {
  done: number;
  total: number;
}

// --- loadprobe report (pmc_blackbox.log analysis) ---

export type Verdict =
  | { kind: "ReachedWorld"; furthest: number; name: string; post_load_crash: number | null }
  | { kind: "Crash"; furthest: number; name: string; eip: number; label: string | null }
  | { kind: "Hang"; furthest: number; name: string; stuck_ms: number; steady_free: number | null }
  | { kind: "Truncated"; furthest: number; name: string };

export interface LogBuildArtifact {
  kind: string;
  name: string;
  hash_type: string;
  sha256: string;
  size: number | null;
}

export interface LogCrashInfo {
  raw_ts: string;
  code: string;
  eip: number;
  eip_label: string | null;
  av: string | null;
  block: string[];
  terminal: boolean;
  since_world_load_ms: number | null;
}

export interface LogReport {
  file: string;
  log_sha256: string;
  build: LogBuildArtifact[];
  records: number;
  first_ts: string;
  last_ts: string;
  wall_ms: number;
  furthest_idx: number;
  furthest_name: string;
  pct: number;
  verdict: Verdict;
  crash: LogCrashInfo | null;
  tail: string[];
  last_progress_ts: string;
  last_progress_msg: string;
  unknown_sources: [string, number][];
  unparsed_lines: number;
  signals: { text: string; count: number; first_ts: string; last_ts: string }[];
}

export interface BuildOptions {
  mods: LoadedMod[];
  excluded_assets: number[];
  output_dir: string;
  split_by_patch: boolean;
  merge_into: string | null;
}

/**
 * User-supplied overrides for Proton/runtime discovery (any field may be
 * omitted or null). Mirrors the Rust `LaunchOverrides` struct, which is
 * `#[serde(rename_all = "camelCase")]`, so keys are camelCase.
 */
export interface RuntimeOverrides {
  steamRoot?: string | null;
  proton?: string | null;
  sniper?: string | null;
  prefix?: string | null;
  useContainer?: boolean | null;
}

/**
 * What runtime discovery resolved to, surfaced to the UI so the user can
 * confirm or override before launching. Mirrors the Rust `RuntimeInfo` struct
 * (`#[serde(rename_all = "camelCase")]`).
 */
export interface RuntimeInfo {
  steamRoot: string | null;
  proton: string | null;
  sniper: string | null;
  /** Whether a launch would run inside the sniper container. */
  container: boolean;
  /** Non-fatal notes (e.g. "no sniper runtime found — will run bare Proton"). */
  notes: string[];
}
