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

/**
 * What happened to one mod's claim group during load-order resolution.
 *
 * A "claim group" is everything one mod contributes, won or lost together — a vehicle
 * reskin is a model plus its textures plus a spawn script, and resolving those
 * individually would give you mod A's model wearing mod B's textures.
 */
export type GroupOutcome =
  | { outcome: "applied"; mod_id: string; label: string; asset_count: number }
  | {
      outcome: "overridden";
      mod_id: string;
      label: string;
      asset_count: number;
      overridden_by_mod: string;
      overridden_by_label: string;
    }
  | {
      outcome: "partially_applied";
      mod_id: string;
      label: string;
      applied: number;
      overridden: number;
    };

/** Two mods overlap partially — neither contains the other. Unresolvable; user must choose. */
export interface ClaimConflict {
  mod_id: string;
  label: string;
  other_mod_id: string;
  other_label: string;
  shared: number[];
  only_mine: number[];
  message: string;
}

export interface BuildResult {
  path: string;
  block_count: number;
  byte_size: number;
  /** sha256 of the bytes written — the only trustworthy way to verify a deploy. */
  sha256: string;
  outcomes: GroupOutcome[];
}

/**
 * A humanoid model this install can actually wear.
 *
 * Only models verified present in the user's own vz.wad are ever offered — the same
 * lookup `Player.SetOutfit` does at runtime — so a pick can't fail in-game, and DLC skins
 * simply don't appear for someone who doesn't own the DLC.
 */
export interface WardrobeModel {
  model: string;
  label: string;
  asset_hash: number;
  /**
   * Fraction of the player characters' skeleton this skin has.
   *
   * These aren't guessed from names any more — a wearable skin is one *rigged to the same
   * skeleton the heroes use*, which is exactly what makes the hero's animations play on it.
   * 1.0 = certain; below that, animation tracks aimed at bones it lacks simply do nothing.
   */
  rig_match: number;
  /** Which of the three heroes it's built most like. */
  closest_hero: string;
  triangles: number;
  /** One of the three player characters, or one of their unlock tiers. */
  is_hero: boolean;
  /** Already in the base game's wardrobe — adding it changes nothing (it's deduped out). */
  in_base_wardrobe: boolean;
}

/** An outfit to add to the PMC wardrobe. */
export interface WardrobeOutfit {
  hero: string; // "mattias" | "chris" | "jennifer"
  model: string;
  label: string;
}

/**
 * One texture in the browsable catalog.
 *
 * The WAD stores only hashes, so this list is the subset of textures whose *name* we can
 * recover — 99.9% of them on a stock install. Cheap to build: no blocks are decompressed.
 */
export interface TextureEntry {
  name: string;
  asset_hash: number;
  /** Leading token of the name (`pmc`, `al`, `city`…) — the game's own grouping. */
  category: string;
  /** `diffuse` | `normal` | `specular` | `other`. */
  kind: string;
}

/** A decoded thumbnail. */
export interface TexturePreview {
  name: string;
  /** `data:image/png;base64,…` */
  data_url: string;
  /** The texture's real in-game size. */
  width: number;
  height: number;
  /**
   * Size of the image we could actually decode. Smaller than width×height for a streamed
   * texture, which keeps only its lowest mips inline — we don't fake the difference.
   */
  preview_width: number;
  preview_height: number;
}

/**
 * A model that samples a texture.
 *
 * `name` is null when we can't recover it — but the WAD addresses models **by hash**, so an
 * unnamed model still loads and renders perfectly. Always pass `reference` (the name, or
 * `0x…`) to the geometry commands; only use `name` for display.
 */
export interface ModelRef {
  hash: number;
  name: string | null;
  reference: string;
}

/** Result of writing a texture out to disk. */
export interface TextureExport {
  path: string;
  width: number;
  height: number;
  full_width: number;
  full_height: number;
  /** False when the game streams this texture's detail, so only a smaller version exists inline. */
  is_full_resolution: boolean;
}

/**
 * Everything the details page shows.
 *
 * `used_by` doesn't exist anywhere in the game files — it's inverted out of every model's
 * MTRL texture slots. The first lookup on an install builds that index (~10s); after that
 * it's cached.
 */
export interface TextureDetails {
  name: string;
  asset_hash: number;
  category: string;
  kind: string;
  width: number;
  height: number;
  format: string;
  chain_bytes: number;
  mip_count: number;
  fully_resident: boolean;
  preview: TexturePreview | null;
  /** Models that actually PAINT this texture — the ones the 3D view can show. */
  used_by: ModelRef[];
  /**
   * Models that reference it in a material but never bind it to a drawable part.
   *
   * Real cases: the geometry belongs to the model's *wreck* variant, or to a separate
   * sub-model (a tank declares its tracks' textures, but the tracks are their own model), or
   * the container is a low-detail variant that merges its parts away.
   */
  declared_only_by: ModelRef[];
  /** More than one model paints it — replacing it changes all of them. */
  shared: boolean;
  /** The other maps of the same surface (`X`, `X_nm`, `X_sm`). */
  siblings: TextureEntry[];
  /** Other textures those same models use — the rest of that character/vehicle. */
  seen_with: TextureEntry[];
}

/**
 * One state/LOD variant of a model, and whether the texture shows up in it.
 *
 * A model's parts are gated by a SEGM state bit. Those bits are not an ordered detail
 * ladder — they're state masks, and a texture can be painted in one state and absent in
 * another. So instead of guessing, the backend builds every state the model declares and
 * reports which ones show the texture; the user can toggle between them.
 */
export interface ModelVariant {
  /** The SEGM state bit (null = every part, unfiltered). */
  tier: number | null;
  groups: number;
  triangles: number;
  highlighted: number;
  shows_texture: boolean;
}

/**
 * One part, anywhere in the game, that paints a given texture.
 *
 * The per-model parts list answers "where is it on *this* model". This answers the broader
 * question — what, across the whole game, is this texture actually on — which is the one that
 * matters before you repaint something 34 models share.
 */
export interface TexturePart {
  /** Pass to the geometry commands (name, or `0x…`). */
  model: string;
  model_name: string | null;
  model_hash: number;
  /**
   * Part id — **valid only together with `tier`**.
   *
   * A part id indexes into the built group list, and that list depends on which state bit was
   * built (Chris is 40 parts unfiltered but 25 at his default state). Pass both back, or you
   * isolate a different part.
   */
  part: number;
  /** The state bit this id belongs to (null = the auto-selected one). */
  tier: number | null;
  triangles: number;
  slot: string;
  lod_mask: number;
}

/** One texture slot of a part's material. */
export interface SlotRef {
  /** `diffuse` | `specular` | `normal` | `map N`. */
  slot: string;
  hash: number;
  name: string | null;
  /** True when this is the texture the page is about. */
  is_current: boolean;
}

/**
 * One draw call — a "part" of the model.
 *
 * A model isn't one mesh: Chris is 25 of these, each binding its own material (eyes, teeth,
 * head, upper body, the pistol he's holding…). Several parts often share one PRMG group,
 * because a PRMG concatenates sub-strips with *different* materials.
 */
export interface GeoGroup {
  id: number;
  index_start: number;
  index_count: number;
  triangles: number;
  uses_texture: boolean;
  /** Which MTRL slot matched: `diffuse` | `specular` | `normal` | `map N`. */
  slot: string | null;
  diffuse: number | null;
  /** Every texture this part wears, named where we can. */
  textures: SlotRef[];
  /** The container's PRMG drawing-group index (several parts can share one). */
  prmg: number;
  /** Which state/LOD bits this part is drawn in. */
  lod_mask: number;
  /** HIER node it hangs off (negative = none). */
  node: number;
}

/**
 * A model flattened for three.js.
 *
 * The highlight is exact, not guessed: the geometry decoder splits the model into draw
 * groups and each records the texture hashes its material binds, so `uses_texture` is a
 * direct comparison against the texture we're looking at.
 */
export interface ModelGeometry {
  model: string;
  model_hash: number;
  positions: number[];
  normals: number[];
  uvs: number[];
  indices: number[];
  groups: GeoGroup[];
  bbox_min: [number, number, number];
  bbox_max: [number, number, number];
  highlighted_groups: number;
  /**
   * The SEGM state/LOD bit this geometry came from (null = every group, unfiltered).
   *
   * Not every model has the engine's default `0x01` — `vz_hum_deathsquad_a` only declares
   * `0x08` — and the bits are state masks, not an ordered detail ladder, so the backend picks
   * a tier that actually contains the texture rather than assuming one.
   */
  tier: number | null;
}

/** A texture in the game, and whether we can replace it. */
export interface TextureTarget {
  name: string;
  asset_hash: number;
  width: number;
  height: number;
  format: string; // DXT1 | DXT5
  swappable: boolean;
  reason: string | null;
}

/** Replace `name` with the image at `image_path` (resized to the game's dimensions). */
export interface TextureSwap {
  name: string;
  image_path: string;
}

/**
 * A community-made, pre-built `vz-patch.wad` imported into the load order.
 *
 * The game only loads one patch WAD, which is why two such mods have never been
 * installable together. modkit merges them: each WAD's blocks travel with their own ASET
 * rows, and the writer re-derives every block index on output.
 */
export interface PrebuiltWad {
  id: string;
  name: string;
  path: string;
  block_count: number;
  asset_count: number;
  /** Ships a compiled scripts_vz block — cannot be composed with another that does. */
  has_scripts: boolean;
  warnings: string[];
}

/** A snapshot of a `vz-patch.wad` that a deploy displaced. */
export interface WadBackup {
  file: string;
  path: string;
  byte_size: number;
  sha256: string;
}

export interface DeployWadResult {
  installed_at: string;
  sha256: string;
  byte_size: number;
  backed_up: WadBackup | null;
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

/** One Mercenaries2*.exe found in the install, identified by size. */
export interface ExeCandidate {
  path: string;
  name: string;
  size: number;
  version: string; // "v1.0" | "v1.1" | "unknown"
  variant: string; // "unsigned" | "ea-signed" | "patched" | "cracked" | "unknown"
}

export interface GameInfo {
  root: string;
  /** The base exe — apply_crack's input, not necessarily the one we launch. */
  exe_path: string;
  exe_size: number;
  version: string; // "v1.0" | "v1.1" | "unknown"
  variant: string; // "unsigned" | "ea-signed" | "patched" | "cracked" | "unknown"
  /** The de-DRM'd exe next to the base one (Mercenaries2.cracked.exe), if any. */
  cracked_exe: ExeCandidate | null;
  /** The exe launch_game actually runs: the cracked one when present. */
  launch_exe_path: string;
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
  /**
   * True when the Tauri updater can install this update in-place (NSIS install
   * or AppImage with a signed update manifest). False falls back to linking to
   * the release page (portable exe, deb/rpm/flatpak, dev builds).
   */
  canInstall: boolean;
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

/**
 * Matchmaking-relevant registry state. The game keys its multiplayer version off
 * the `Region` value under the EA Games install key; players must share ONE
 * `Region` to see each other in lobbies. The user picks which region's pool
 * they're in (defaulting to the community pool value).
 */
export interface RegionStatus {
  applicable: boolean; // false on non-Windows hosts (key lives in the Wine prefix)
  keyPresent: boolean;
  currentRegion: string | null;
  currentInstallDir: string | null;
  expectedRegion: string; // the user's selected Region (pool default if unset)
  knownRegions: string[]; // all Region values the game recognizes
  installDir: string; // the Install Dir value normalizing would write
  normalized: boolean; // currentRegion already equals expectedRegion
  detail: string;
}

export interface NormalizeRegionResult {
  ok: boolean;
  region: string;
  message: string;
}

/** Presence/size of one language's content (`<Lang>.wad` + `vo_stream.<lang>.pws`). */
export interface LanguagePresence {
  language: string;
  locales: string[];
  wadName: string;
  wadPresent: boolean;
  wadSize: number;
  pwsName: string;
  pwsPresent: boolean;
  pwsSize: number;
}

/** Which languages the install currently carries. */
export interface LanguageStatus {
  dataDir: string | null;
  audioDir: string | null;
  languages: LanguagePresence[];
  presentCount: number;
}

/** Result of keeping one language and trashing the others. */
export interface SetLanguageResult {
  kept: string;
  removed: string[]; // basenames moved to the recoverable trash
  freedBytes: number;
  trashDir: string | null;
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

/** One save-game `.profile` file, with parsed header details when readable. */
export interface SaveFileInfo {
  file_name: string;
  size: number;
  modified_unix: number;
  /** The game's rolling `auto_*` slot (overwritten constantly by mission Lua). */
  autosave: boolean;
  character: string | null;
  cash: number | null;
  playtime_seconds: number | null;
  saved_at_unix: number | null;
  last_mission: string | null;
}

/** The live SaveGames folder and its contents. */
export interface SavesInfo {
  dir: string | null;
  /** True when `dir` is the user's saved override, not the autodetected path. */
  overridden: boolean;
  exists: boolean;
  saves: SaveFileInfo[];
}

/** One stored snapshot of the SaveGames folder. */
export interface SaveBackupInfo {
  id: string;
  reason: string; // "pre-launch" | "pre-restore" | "manual"
  created_unix: number;
  file_count: number;
  total_bytes: number;
  characters: string[];
}

/** Result of a snapshot attempt (id null when skipped, with the reason why). */
export interface BackupResult {
  id: string | null;
  skipped: string | null;
  file_count: number;
}

/** Result of restoring a snapshot over the live saves. */
export interface RestoreResult {
  restored: string[];
  pre_restore_backup: string | null;
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
