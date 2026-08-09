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

/**
 * The kind of place an entry in the load order came from. Closed vocabulary — mirrors Rust's
 * `models::origin::OriginSource`.
 */
export type OriginSource =
  /** Installed through mercs.ink; carries a full public id. */
  | "registry"
  /** Installed from a mod-source repository index (a {@link CatalogMod}). */
  | "catalog"
  /** A hand-built source folder staged from disk. User-named, so no id. */
  | "local"
  /** A local file the user picked (a prebuilt WAD, a loose `.asi`). User-named, so no id. */
  | "imported"
  /** Something modkit synthesizes rather than installs — the wardrobe, the texture queue. */
  | "modkit";

/**
 * Where an entry came from, captured at ingest/install time.
 *
 * It has to be captured *then* because it cannot be recovered later. A catalog install used to
 * keep nothing that pointed back at its {@link CatalogMod} — for a WAD mod not the repository,
 * the slug or the release tag, and for an ASI only a lossy `slugify("{repo}-{slug}")` that
 * cannot be parsed back into the pair. Re-associating an installed plugin with its catalogue
 * entry was done by matching `.asi` filenames, which is a guess.
 *
 * **`id` is comparable only against another id with the same `source`.** No slug identifies a
 * mod on its own: a registry slug is the Shipment's declared name, which every fork of it
 * shares, and a catalog slug is unique only within its repository. Both namespaces are
 * composite. `version` splits the same way — a GitHub release tag for `catalog`, a manifest
 * version for `registry`/`local`.
 *
 * `null` on either field is a bucket with a meaning (an unreleased mod, a folder the user
 * named), not missing data to be filled in with a guess.
 */
export interface Origin {
  source: OriginSource;
  /** Source-scoped identity, or null when the entry has no public one. */
  id: string | null;
  /** Release tag or manifest version; null is meaningful. */
  version: string | null;
}

/** The wardrobe's fixed origin id — modkit synthesizes one outfit Shipment from every pick. */
export const MODKIT_WARDROBE_ID = "modkit:wardrobe";
/** The texture queue's fixed origin id — swaps enter the WAD like any other asset claim. */
export const MODKIT_TEXTURES_ID = "modkit:textures";

export interface LoadedMod {
  id: string;
  root: string;
  manifest: Manifest;
  assets: DetectedAsset[];
  /**
   * Captured by the store at install time; the `load_mod` command that builds the rest of this
   * object never sees a catalogue. Absent on rows persisted before origins existed — and left
   * absent, because a heuristic backfill would assert a provenance nothing recorded.
   */
  origin?: Origin;
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

/**
 * A loose file a Shipment places into the **game folder** — an `.asi` plugin from a `native_hook`
 * contribution, or a companion from a `place_file` one. Not WAD content: it is staged beside
 * `vz-patch.wad` and copied into the game install by the deploy step.
 */
export interface StagedFile {
  /** Absolute path to the staged file in the build directory. */
  source: string;
  /** Destination under the game folder, forward-slashed. */
  relative: string;
  sha256: string;
  /** Which Shipment placed it. */
  shipment: string;
}

export interface BuildResult {
  /** The built WAD, or `""` when the load order produced no blocks at all (a Shipment carrying
   *  only `native_hook` / `place_file` contributions is a real build with no WAD in it). */
  path: string;
  /** The build output directory. Deploy reads its `placement.json`. */
  staging_dir: string;
  block_count: number;
  byte_size: number;
  /** sha256 of the bytes written — the only trustworthy way to verify a deploy. */
  sha256: string;
  outcomes: GroupOutcome[];
  /** Non-fatal advisories from assembly (e.g. a Shipment's scripts override the wardrobe's). */
  warnings?: string[];
  /** Files that will be dropped into the game folder on install. An `.asi` is native code. */
  placed_files?: StagedFile[];
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

/**
 * An outfit to add to the PMC wardrobe.
 *
 * Origin is per-collection, not per-row: every pick is folded into one synthesized Shipment, so
 * the whole `wardrobe` array contributes as a single `modkit` entry under
 * {@link MODKIT_WARDROBE_ID}.
 */
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

/**
 * Replace `name` with the image at `image_path` (resized to the game's dimensions).
 *
 * Swaps feed the WAD build like every other collection, so one can collide with any other claim
 * on the same asset — the queue is a load-order contributor in its own right, under
 * {@link MODKIT_TEXTURES_ID}. Origin is per-collection, not per-row.
 */
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
  /** Always `imported` with a null id: a file the user picked and named. */
  origin?: Origin;
}

/**
 * A Workshop **Shipment** (a Quartermaster source project) staged in the load order.
 *
 * Unlike {@link PrebuiltWad}, this is source, not a finished WAD: modkit builds and Lua-links it
 * through `qm` at assemble time, so several script-touching Shipments reconcile instead of one
 * clobbering another.
 */
export interface ShipmentRef {
  /**
   * Local dedupe key and load-order row id, derived from the folder name. Deliberately *not*
   * the Shipment's identity: it has to tell two checkouts of the same Shipment apart, which a
   * slug — shared by every fork — cannot. It never leaves the machine.
   */
  id: string;
  /** The Shipment's declared name once the manifest is read; the folder name only if it wasn't. */
  name: string;
  path: string;
  /**
   * `shipment.name` from the qm manifest — the Shipment's declared slug, and **half** its
   * identity. The other half is the repository, which a folder staged from disk cannot know.
   * `null` when the manifest could not be parsed.
   */
  slug: string | null;
  /** `shipment.version` from the manifest. `null` is meaningful — an unreleased mod. */
  version: string | null;
  /** `local` with a null id for anything staged from disk. */
  origin: Origin;
}

/** A snapshot of a `vz-patch.wad` that a deploy displaced. */
export interface WadBackup {
  file: string;
  path: string;
  byte_size: number;
  sha256: string;
}

/** One loose file a deploy put into the game folder. */
export interface PlacedFile {
  abs_path: string;
  relative: string;
  sha256: string;
  shipment: string;
}

/** What the loose-file half of a deploy (or an uninstall) did. */
export interface PlacementOutcome {
  placed: PlacedFile[];
  /** Moved to the recoverable trash. */
  removed: string[];
  /** Left alone: the bytes no longer match what modkit wrote, so somebody replaced them by hand. */
  skipped: string[];
  /** Pre-existing unmanaged files displaced to `<name>.bak`. */
  backed_up: string[];
}

export interface DeployWadResult {
  /** Empty when the build carried no WAD — the installed patch is then left untouched. */
  installed_at: string;
  sha256: string;
  byte_size: number;
  backed_up: WadBackup | null;
  files: PlacementOutcome;
}

/**
 * The `vz-patch.wad` modkit currently has installed — a durable record that survives a restart,
 * unlike {@link DeployWadResult}, which was returned and discarded.
 *
 * Distinct from {@link WadBackup}: a backup describes a WAD that was **displaced**, this
 * describes the live one. `null` from `deployed_wad_record` means no patch is deployed, which is
 * a real state (an ASI-only setup has never deployed one) and not an error.
 *
 * It states what modkit last wrote. A user can replace `vz-patch.wad` behind modkit's back, so
 * anything needing certainty re-hashes the file at `installed_at`.
 */
export interface DeployedWadRecord {
  installed_at: string;
  /** sha256 of the deployed bytes. */
  sha256: string;
  byte_size: number;
  /** Unix epoch seconds at deploy time. Ordering only; never an identifier. */
  deployed_at: number;
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
  /** dxwrapper.dll present — the non-destructive loader for licensed copies. */
  has_dxwrapper: boolean;
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

// ---------------------------------------------------------------------------------------
// mercs.ink — the community registry (mirrors Rust's `commands::mercsink`).
//
// A separate namespace from {@link CatalogMod} on purpose, and never merged with it. A
// registry mod is identified by {@link RegistryMod.id}; a catalog mod by `"repo-url#slug"`.
// Those are not comparable, so the two lists are shown side by side and labelled rather than
// interleaved — putting one id next to the other invites a comparison that means nothing.
//
// Every interface here is a *head*: only the fields modkit uses. The API is additive-only and
// clients are required to ignore unknown keys, which is what lets the server grow a field
// without breaking installed copies.
// ---------------------------------------------------------------------------------------

/**
 * One downloadable file on a release. Hosted by GitHub — mercs.ink never re-hosts artifacts.
 *
 * The server also sends `download_count` for its own author dashboard; it is omitted here
 * rather than mirrored unused.
 */
export interface ReleaseAsset {
  name: string;
  download_url: string;
  /**
   * Bytes, as GitHub reported them. **Not an integrity check** — the API carries no checksum
   * for an asset, because it caches release metadata rather than the artifact and so has
   * nothing to attest to.
   *
   * A manifest's `load.requires` can carry a `sha256`, but that is the manifest author's claim
   * about an external URL, not the registry's claim about this asset. Different bytes,
   * different trust statement; they are not interchangeable.
   */
  size: number | null;
  content_type: string | null;
}

/** The head of a parsed Quartermaster manifest, as the registry serves it. */
export interface ManifestHead {
  format: number | null;
  shipment: {
    /** `shipment.name` — the declared slug, and half an identity: forks share it. */
    name: string | null;
    title: string | null;
    /** `shipment.version` — the namespace a `registry` origin's version lives in. */
    version: string | null;
    target: string | null;
  };
}

/** One synced release of a registered mod. */
export interface RegistryRelease {
  version: string;
  tag: string | null;
  published_at: string | null;
  /**
   * qm's `Target` — `retail` | `reimpl` | `both`. A **shipment compatibility** declaration,
   * and not the crash report's `game.target`, which says what was actually running. A Shipment
   * declaring `both` can appear in a convoy whose `game.target` is `retail`.
   */
  target: string | null;
  /** The manifest format. A release declaring more than modkit supports is refused. */
  format: number | null;
  assets: ReleaseAsset[];
  /** Served already parsed, so modkit never re-reads the YAML out of the artifact. */
  manifest: ManifestHead | null;
}

/** One mod on mercs.ink. */
export interface RegistryMod {
  /**
   * mercs.ink's stable public identifier, precomposed server-side and **opaque**. File records
   * under it; never take it apart and never rebuild it from a slug and a repo id, because two
   * implementations of one identity format drift, and the day they disagree a mod's history
   * splits into two buckets where the drop reads as a fix.
   *
   * `null` against a deployment older than the field — an absence, not something to fill in.
   */
  id: string | null;
  /** `shipment.name`. Not an identity on its own: every fork of a mod declares the same one. */
  slug: string;
  title: string | null;
  description: string | null;
  /** qm's `Target`, as on {@link RegistryRelease.target} — compatibility, not the game. */
  target: string | null;
  tags: string[];
  authors: string[];
  homepage: string | null;
  license: string | null;
  /** Display only. Owner-derived, so a rename or transfer changes it — never a key. */
  repository: string | null;
  latest_version: string | null;
  latest_release: RegistryRelease | null;
}

/**
 * A registry read, with the staleness the UI has to disclose.
 *
 * Both halves travel together because the recommended flow requires both at once: when
 * mercs.ink cannot be reached, show the cached catalogue *and* say it is cached. Returning only
 * the data would make "the registry is down" indistinguishable from "nothing changed".
 */
export interface RegistryFeed {
  mods: RegistryMod[];
  /** True when `mods` came from the local cache because the server could not answer. */
  stale: boolean;
  /** A user-facing explanation for a banner. `null` when the fetch succeeded. */
  warning: string | null;
}

/** The outcome of installing a Shipment from mercs.ink. */
export interface MercsInkInstall {
  /** The load-order entry, carrying a `registry` origin with the registry's own id. */
  shipment: ShipmentRef;
  slug: string;
  title: string | null;
  release_version: string;
  /** qm's `Target` for this release — shipment compatibility, not `game.target`. */
  target: string | null;
  assets: string[];
  staged_files: number;
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
  /** Local row id. Lossy — `slugify("{repo}-{slug}")` cannot be parsed back; see {@link origin}. */
  id: string;
  name: string;
  description: string;
  version: string;
  modRoot: string;
  asiFiles: string[];
  /**
   * `catalog` for a catalogue download, `imported` for a locally-picked `.asi`. This is the gap
   * that mattered most: an ASI mod kept nothing else pointing back at its {@link CatalogMod},
   * which is why re-association is still done by matching `.asi` basenames.
   */
  origin?: Origin;
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

/**
 * One binary from the Workshop toolset (the mercs2-wad-simulator release), as
 * shown on the Workshop Tools page.
 */
export interface ToolStatus {
  /** Asset stem, e.g. "wad_simulator". Also the id used by install/uninstall. */
  name: string;
  label: string;
  blurb: string;
  /**
   * A windowed program you launch (the Workshop, the native game) rather than a
   * command-line tool. Independent of `driven_by_modkit`.
   */
  windowed: boolean;
  /** Modkit shells out to this tool itself, so removing it breaks a feature. */
  driven_by_modkit: boolean;
  /**
   * False when the release publishes no build for this machine — the
   * engine-backed apps (Workshop, Game) are 64-bit only, and there are no assets
   * at all for unsupported OS/arch pairs.
   */
  available: boolean;
  /** Absolute path once installed, else null. */
  path: string | null;
  /** The executable's size only — a companion bundle is not counted. */
  size: number | null;
  /** Not yet faithful to the retail game — offered for testing, not playing. */
  experimental: boolean;
  /**
   * This tool cannot start without a Mercenaries 2 install — outside Windows it
   * has no registry key to fall back on.
   */
  requires_game_dir: boolean;
  /**
   * Data bundle this tool needs unpacked beside it, or null if it needs none.
   * The Workshop reads its reference data from `workshop_data/` next to its exe.
   */
  companion_dir: string | null;
  /** False alongside a non-null `path` means a half-finished install. */
  companion_ready: boolean;
}

/**
 * Toolset-wide status. The whole toolset ships in ONE release, so it has a
 * single installed tag rather than a version per tool.
 */
export interface ToolsetStatus {
  installed_tag: string | null;
  /** Null when the lookup was skipped or failed (offline). */
  latest_tag: string | null;
  update_available: boolean;
  /**
   * Directory "Open folder" opens — the version directory once something is
   * installed, otherwise the toolset root. Always a real, existing directory.
   */
  dir: string;
  tools: ToolStatus[];
}

/** A tool modkit launched that exited badly. Reported once, on the next poll. */
export interface ToolFailure {
  name: string;
  label: string;
  /** Exit status plus the tail of that tool's log. */
  message: string;
}

/** Liveness snapshot for the tools modkit started. */
export interface ToolsRunning {
  /** Names of tools still running. */
  running: string[];
  /** Crashes since the previous poll — drained server-side, so never repeated. */
  failures: ToolFailure[];
}

/** Progress event emitted by the backend while installing the toolset. */
export interface ToolsetProgress {
  tool: string;
  label: string;
  done: number;
  total: number;
}

/**
 * The three independent features a pmc_bb build may carry. Upstream publishes one
 * subset per release asset, so these — not a filename — are what modkit selects on.
 */
export interface PmcBbFeatures {
  /** SecuROM v7 event spoof. Only cracked exes that import pmc_bb.dll want this. */
  crack: boolean;
  /** The ASI loader. When absent, something else must scan for plugins. */
  asi: boolean;
  /** Log stack, crash handler, Lua hooks — what every diagnostic is built on. */
  log: boolean;
}

/** One published pmc_bb build, for the advanced picker. */
export interface PmcBbVariant {
  asset: string;
  features: PmcBbFeatures;
  blurb: string;
}

/** Which build modkit would install here, and why. */
export interface PmcBbChoice {
  asset: string;
  features: PmcBbFeatures;
  /** Plain-language reason, safe to show verbatim. */
  reason: string;
  /** True when the user forced a build rather than modkit choosing. */
  overridden: boolean;
}

export interface InstallDllResult {
  path: string;
  version: string;
  /**
   * The release asset installed, e.g. `pmc_bb_asi_log.dll`. Every build installs
   * as `pmc_bb.dll` — the name the exe's import table and dxwrapper's
   * LoadCustomDllPath resolve — so this is the only thing that says which one.
   */
  asset: string;
  features: PmcBbFeatures;
  reason: string;
  overridden: boolean;
}

/** Result of installing the shared m2 SDK runtime (`m2-sdk.dll`) into the game root. */
export interface InstallM2SdkResult {
  path: string;
  version: string;
}

/** One managed dependency after an auto-on-deploy resolution pass. */
export interface ResolvedDependency {
  name: string;
  versionReq: string;
  /** Release tag installed, or null when the dependency was skipped (see `note`). */
  installedTag: string | null;
  /** Why it was skipped, when it was. */
  note: string | null;
}

/** Result of resolving a Shipment's managed `load.requires` on deploy. */
export interface ResolveDepsResult {
  resolved: ResolvedDependency[];
}

/**
 * Install state for one artifact modkit manages, from the backend ledger.
 *
 * Replaces the per-component version strings that used to live in localStorage,
 * which could not survive a cleared profile, were never checked against disk, and
 * could not say which of six pmc_bb builds was installed.
 */
export interface ComponentStatus {
  key: string; // "pmc_bb" | "dxwrapper" | "apply_crack"
  label: string;
  repo: string;
  /** Release tag modkit installed; null when modkit did not install it. */
  installedTag: string | null;
  /** Release asset installed — which variant. */
  installedAsset: string | null;
  features: string[];
  /** Latest published tag; null when the lookup was skipped or failed. */
  latestTag: string | null;
  updateAvailable: boolean;
  /** Every recorded file is still on disk. */
  present: boolean;
  /** Still there, but no longer the bytes modkit wrote — replaced by hand. */
  modified: boolean;
  url: string | null;
}

export interface CrackResult {
  ok: boolean;
  output_path: string;
  stdout: string;
  stderr: string;
  tool_version: string; // apply_crack release tag that was downloaded & run
}

/**
 * Whether this machine holds a SecuROM activation for the game. When `licensed`,
 * Setup offers the non-destructive dxwrapper path instead of cracking the exe.
 */
export interface LicenseStatus {
  applicable: boolean; // false on non-Windows (activation lives in the Wine prefix)
  licensed: boolean; // legally owned: CD-Key registered OR SecuROM-activated
  cdKeyPresent: boolean; // EA ergc CD-Key present (Mercs2-specific proof of purchase)
  licenseKeyPresent: boolean; // "License information - Do not delete!" present
  userDataPresent: boolean; // UserData holds securom_v7_* activation data (main signal)
  keysFound: string[]; // the exact registry keys that matched
  detail: string;
}

/** Outcome of installing dxwrapper as the licensed-copy mod loader. */
export interface DxwrapperResult {
  ok: boolean;
  version: string; // dxwrapper release tag installed
  proxyPath: string; // the stub proxy DLL written (d3d9.dll)
  dxwrapperPath: string;
  iniPath: string;
  /**
   * Whether dxwrapper was configured to scan for plugins itself (`LoadPlugins=1`).
   * Derived from the pmc_bb build this install gets: exactly one of the two owns
   * scanning, so when pmc_bb has no ASI loader compiled in, dxwrapper takes over.
   */
  loadsPlugins: boolean;
  /** The pmc_bb build that decision was made against. */
  pmcBbAsset: string;
  notes: string[];
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

/** A NOVEL language installed as `data/<name>.wad` — one the base game never shipped. */
export interface AddedLanguage {
  name: string; // the WAD basename / language token (e.g. "polski")
  display: string; // friendlier label (title-cased)
  wadName: string;
  wadSize: number;
  active: boolean; // the selector is enabled AND names this language
}

/** State of the `mercs2_language` selector plugin that switches into an added language. */
export interface SelectorStatus {
  pluginInstalled: boolean;
  enabled: boolean;
  active: string | null; // the name the config selects, enabled or not
  dryRun: boolean;
}

/** Which languages the install currently carries. */
export interface LanguageStatus {
  dataDir: string | null;
  audioDir: string | null;
  languages: LanguagePresence[];
  presentCount: number;
  added: AddedLanguage[];
  selector: SelectorStatus;
}

/** Result of keeping one language and trashing the others. */
export interface SetLanguageResult {
  kept: string;
  removed: string[]; // basenames moved to the recoverable trash
  freedBytes: number;
  trashDir: string | null;
}

/** Result of selecting or clearing an added language. */
export interface SetAddedLanguageResult {
  name: string | null; // the language now selected, or null when cleared
  iniPath: string;
  enabled: boolean;
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
  /**
   * The catalogue's stable id for the matched build (`v11-cracked-pmcbb`, …), or null when no
   * entry has this md5.
   *
   * Null is a real answer, not a prompt to fall back on the nearest size: two catalogued builds
   * are byte-for-byte the same length, so a size guess would pool one build's records with
   * another's. This is also why the id exists at all — `(version, variant)` cannot separate
   * those two, and neither can any other descriptive tuple, since each one guesses at whichever
   * attributes happen to differ among today's builds.
   */
  identifiedId: string | null;
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
