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
  asi_loader_proxy: string | null; // e.g. "dinput8.dll", or null if none
  data_dir: string | null;
  deployed_patches: string[];
  deployed_asi: string[];
}

export interface CatalogEntry {
  name: string;
  description: string;
  repository: string;
}

export interface Catalog {
  entries: CatalogEntry[];
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

export interface InstallDllResult {
  path: string;
  version: string;
}

export interface CrackResult {
  ok: boolean;
  output_path: string;
  stdout: string;
  stderr: string;
}

export interface BuildOptions {
  mods: LoadedMod[];
  excluded_assets: number[];
  output_dir: string;
  split_by_patch: boolean;
  merge_into: string | null;
}
