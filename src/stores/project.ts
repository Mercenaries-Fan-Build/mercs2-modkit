import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { check as checkUpdate, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import type {
  AsiMod,
  BuildResult,
  CatalogMod,
  Catalog,
  RepoSource,
  ComponentUpdate,
  ConflictGraph,
  CrackResult,
  LicenseStatus,
  DxwrapperResult,
  DeployedAsi,
  DeployResult,
  DeployWadResult,
  ExeCandidate,
  GameInfo,
  InstallDllResult,
  InstallResult,
  LoadedMod,
  LogReport,
  ModelGeometry,
  ModelVariant,
  ModkitUpdate,
  PrebuiltWad,
  ReleaseInfo,
  Resolution,
  RuntimeInfo,
  RuntimeOverrides,
  SaveBackupInfo,
  SavesInfo,
  TextureDetails,
  TextureEntry,
  TextureExport,
  TexturePart,
  TexturePreview,
  TextureSwap,
  TextureTarget,
  ToolsetProgress,
  ToolsetStatus,
  ToolsRunning,
  BackupResult,
  RestoreResult,
  TrashResult,
  ValidationResult,
  WadBackup,
  WardrobeModel,
  WardrobeOutfit,
  VcRedistStatus,
  InstallVcRedistResult,
  VerifyReport,
  GenerateManifestResult,
  DebugZipResult,
  RegionStatus,
  NormalizeRegionResult,
  LanguageStatus,
  SetLanguageResult,
} from "../types";

const GAME_PATH_KEY = "mercs2-modkit:gamePath";
const ASI_TARGET_KEY = "mercs2-modkit:asiTarget";
const LIBRARY_KEY = "mercs2-modkit:library";
// Versions of the core components modkit last installed, remembered so a later
// release of either can be flagged as an available update.
const PMC_BB_VERSION_KEY = "mercs2-modkit:pmcBbVersion";
const CRACK_VERSION_KEY = "mercs2-modkit:crackVersion";
const DXWRAPPER_VERSION_KEY = "mercs2-modkit:dxwrapperVersion";
const REGION_KEY = "mercs2-modkit:preferredRegion";

function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/** The `.asi` asset basenames a catalog mod deploys. */
function catalogAsiNames(item: CatalogMod): string[] {
  return item.assets
    .filter((a) => a.toLowerCase().endsWith(".asi"))
    .map((a) => a.split(/[\\/]/).pop() ?? a);
}

/** Parse a loose semver-ish string ("v0.2.0", "1.10") into numeric parts. */
function parseVer(v: string): number[] {
  return v
    .replace(/^v/i, "")
    .split(".")
    .map((x) => parseInt(x, 10) || 0);
}

/** True if version `a` is strictly newer than `b`. */
function semverGt(a: string, b: string): boolean {
  const A = parseVer(a);
  const B = parseVer(b);
  const n = Math.max(A.length, B.length);
  for (let i = 0; i < n; i++) {
    const x = A[i] ?? 0;
    const y = B[i] ?? 0;
    if (x > y) return true;
    if (x < y) return false;
  }
  return false;
}

/** The catalog mod backing a Library ASI mod, matched by `.asi` filename. */
function findCatalogForLib(
  catalog: CatalogMod[],
  mod: AsiMod
): CatalogMod | undefined {
  const asis = mod.asiFiles.map((f) => f.split(/[\\/]/).pop() ?? f);
  return catalog.find((c) => catalogAsiNames(c).some((a) => asis.includes(a)));
}

/** Repository whose releases drive modkit's own self-update check. */
const MODKIT_REPO = "https://github.com/Mercenaries-Fan-Build/mercs2-modkit";

/**
 * Pending in-place update from the Tauri updater. Kept outside Pinia state:
 * it's a live handle with methods, not serializable data.
 */
let pendingUpdate: Update | null = null;
/** Repos publishing the core components modkit installs (release-checked too). */
const PMC_BB_REPO = "https://github.com/Mercenaries-Fan-Build/pmc-blackbox";
const CRACK_REPO = "https://github.com/Mercenaries-Fan-Build/mercs2-securom-bypass";
const DXWRAPPER_REPO = "https://github.com/elishacloud/dxwrapper";

interface ProjectState {
  // Base game
  gamePath: string | null;
  gameInfo: GameInfo | null;
  /** Whether the game instance modkit launched is currently running. */
  gameRunning: boolean;
  // WAD-asset mods — array order is the load order (top wins conflicts).
  mods: LoadedMod[];
  // ASI-plugin mods (deployed into the game's ASI loader folder).
  asiMods: AsiMod[];
  /** mod id -> enabled (defaults to true). Shared across both mod kinds. */
  enabled: Record<string, boolean>;
  // Mod catalog (per-mod rows expanded from repository sources)
  catalog: CatalogMod[];
  catalogSource: string | null;
  // User-added custom mod-source repositories (persisted on disk via Rust).
  customSources: RepoSource[];
  // modkit self-update (vs its GitHub releases)
  modkitUpdate: ModkitUpdate | null;
  /** In-place update download/install in progress. */
  updateInstalling: boolean;
  /** Download progress 0-100, or null when the size is unknown. */
  updateProgress: number | null;
  // Versions of the core components modkit last installed (null = unknown).
  pmcBbVersion: string | null;
  crackVersion: string | null;
  /** dxwrapper release tag last installed (licensed path); null = unknown. */
  dxwrapperVersion: string | null;
  // Release-update status per core component, keyed "pmc_bb" / "apply_crack".
  componentUpdates: Record<string, ComponentUpdate>;
  // Workshop toolset (the mercs2-wad-simulator release binaries modkit manages).
  toolset: ToolsetStatus | null;
  /** An install/update is running; holds which tool is in flight. */
  toolsetProgress: ToolsetProgress | null;
  /** Names of windowed tools modkit started that are still running. */
  runningTools: string[];
  // Host's 32-bit VC++ 2008 runtime status (null = not yet checked).
  vcRedist: VcRedistStatus | null;
  // Matchmaking region registry status (null = not yet checked / N/A).
  region: RegionStatus | null;
  // SecuROM activation / licensing status (null = not yet checked / N/A). When
  // licensed, Setup uses the dxwrapper path and leaves the exe untouched.
  license: LicenseStatus | null;
  // The Region the user picked (persisted; null = the community pool default).
  preferredRegion: string | null;
  // Player saves + stored snapshots (Save backups view).
  savesInfo: SavesInfo | null;
  saveBackups: SaveBackupInfo[];
  savesBusy: boolean;
  // Settings
  asiTarget: string; // ".", "scripts", "plugins", "update"
  // Conflicts & build
  conflictGraph: ConflictGraph | null;
  resolutions: Record<string, Resolution>;
  buildResult: BuildResult | null;
  /** Snapshots of every vz-patch.wad a deploy has displaced — the undo list. */
  wadBackups: WadBackup[];
  /** Wearable models verified present in THIS install. */
  wardrobeModels: WardrobeModel[];
  /** Outfits the user has queued for the next build. */
  wardrobe: WardrobeOutfit[];
  /** Imported community patch WADs, in load order (later wins). */
  prebuilt: PrebuiltWad[];
  /** Texture replacements queued for the next build. */
  textures: TextureSwap[];
  /** Every nameable texture in this install (browsable). Not persisted — cheap to rebuild. */
  textureCatalog: TextureEntry[];
  validation: ValidationResult | null;
  busy: boolean;
  error: string | null;
}

export const useProjectStore = defineStore("project", {
  state: (): ProjectState => ({
    gamePath: null,
    gameInfo: null,
    gameRunning: false,
    mods: [],
    asiMods: [],
    enabled: {},
    catalog: [],
    catalogSource: null,
    customSources: [],
    modkitUpdate: null,
    updateInstalling: false,
    updateProgress: null,
    pmcBbVersion: null,
    crackVersion: null,
    dxwrapperVersion: null,
    componentUpdates: {},
    toolset: null,
    toolsetProgress: null,
    runningTools: [],
    vcRedist: null,
    region: null,
    license: null,
    preferredRegion: null,
    savesInfo: null,
    saveBackups: [],
    savesBusy: false,
    asiTarget: "scripts",
    conflictGraph: null,
    resolutions: {},
    buildResult: null,
    wadBackups: [],
    wardrobeModels: [],
    wardrobe: [],
    prebuilt: [],
    textures: [],
    textureCatalog: [],
    validation: null,
    busy: false,
    error: null,
  }),

  getters: {
    modById: (state) => (id: string) => state.mods.find((m) => m.id === id),
    isEnabled: (state) => (id: string) => state.enabled[id] !== false,
    enabledMods(state): LoadedMod[] {
      return state.mods.filter((m) => state.enabled[m.id] !== false);
    },
    activeAssetCount(): number {
      return this.enabledMods.reduce((n, m) => n + m.assets.length, 0);
    },
    conflictCount: (state) => state.conflictGraph?.conflicts.length ?? 0,
    unresolvedCount(state): number {
      const conflicts = state.conflictGraph?.conflicts ?? [];
      return conflicts.filter((c) => !state.resolutions[String(c.asset_hash)])
        .length;
    },
    gameReady(state): boolean {
      const g = state.gameInfo;
      return !!g && g.version !== "unknown";
    },
    /** Host is missing the 32-bit VC++ 2008 runtime the game needs to launch. */
    vcRedistMissing(state): boolean {
      const v = state.vcRedist;
      return !!v && v.applicable && !v.installed;
    },
    /** Region applies here but isn't the selected value — matchmaking is segregated. */
    regionNeedsNormalize(state): boolean {
      const r = state.region;
      return !!r && r.applicable && !r.normalized;
    },
    /**
     * The v1.1 cracked build we'll launch, whether that's the base exe (cracked
     * in place) or the `Mercenaries2.cracked.exe` setup wrote next to it. The
     * user does NOT have to overwrite the original for the install to be ready.
     */
    crackedBuild(state): ExeCandidate | null {
      const g = state.gameInfo;
      if (!g) return null;
      if (g.cracked_exe?.version === "v1.1" && g.cracked_exe.variant === "cracked") {
        return g.cracked_exe;
      }
      if (g.version === "v1.1" && g.variant === "cracked") {
        return {
          path: g.exe_path,
          name: g.exe_path.split(/[\\/]/).pop() ?? g.exe_path,
          size: g.exe_size,
          version: g.version,
          variant: g.variant,
        };
      }
      return null;
    },
    /**
     * A legitimately licensed copy (SecuROM activation present). Setup steers
     * such installs to the dxwrapper path and never touches the exe.
     */
    isLicensed(state): boolean {
      return state.license?.applicable === true && state.license.licensed;
    },

    /**
     * The exe we'd launch is already DRM-free — an apply_crack output OR a
     * legitimately-owned DRM-free build (e.g. mercs2_nodrm_v3.exe). Such an exe
     * imports pmc_bb.dll itself, so it needs neither a crack (done) nor dxwrapper
     * (not the loader) — just pmc_bb.dll present. `crackedBuild` already captures
     * "a v1.1 DRM-free build is the base or a sibling", by size class.
     */
    exeDrmFree(): boolean {
      return !!this.crackedBuild;
    },

    /**
     * Which of the three setup paths applies, ordered by what would actually
     * launch (mirrors `launch::launch_exe`):
     *   1. dxwrapper installed → "licensed": it launches the stock exe untouched,
     *      so respect that even if a DRM-free sibling is also lying around;
     *   2. else a DRM-free exe would launch → "drm_free": just install pmc_bb.dll;
     *   3. else SecuROM retail exe with an activation → "licensed": set up dxwrapper;
     *   4. else → "crack": apply the bypass.
     */
    setupPath(state): "drm_free" | "licensed" | "crack" {
      if (state.gameInfo?.has_dxwrapper) return "licensed";
      if (this.exeDrmFree) return "drm_free";
      if (this.isLicensed) return "licensed";
      return "crack";
    },

    /** Licensed path ready: dxwrapper + the logging pmc_bb bridge are installed. */
    dxwrapperReady(state): boolean {
      return !!state.gameInfo?.has_dxwrapper && !!state.gameInfo?.has_pmc_bb;
    },

    /**
     * Fully prepared for modding, per path:
     *   - drm_free: pmc_bb.dll present (the exe imports it);
     *   - licensed: dxwrapper + pmc_bb (logging) installed, exe untouched;
     *   - crack:    not set up yet — cracking produces a DRM-free exe, which
     *               flips the path to "drm_free".
     */
    gameFullySetUp(state): boolean {
      switch (this.setupPath) {
        case "drm_free":
          return !!state.gameInfo?.has_pmc_bb;
        case "licensed":
          return this.dxwrapperReady;
        default:
          return false;
      }
    },
    /** Filenames of ASI plugins currently present in the game install. */
    deployedAsiNames(state): Set<string> {
      return new Set((state.gameInfo?.deployed_asi ?? []).map((a) => a.name));
    },
    /** The catalog mod offering a newer version than the installed `mod`, if any. */
    asiUpdate(state) {
      return (mod: AsiMod): CatalogMod | undefined => {
        const cat = findCatalogForLib(state.catalog, mod);
        if (cat && cat.version && mod.version && semverGt(cat.version, mod.version)) {
          return cat;
        }
        return undefined;
      };
    },
    /** WAD-asset mods that declare a dependency on the mod named `name`. */
    dependentsOf(state) {
      return (name: string): LoadedMod[] =>
        state.mods.filter((m) =>
          m.manifest.dependencies.some(
            (d) => d.split("@")[0].trim() === name
          )
        );
    },
    /** Whether a deployed plugin filename is already managed in the Library. */
    isAsiManaged(state) {
      return (name: string): boolean =>
        state.asiMods.some((m) =>
          m.asiFiles.some((f) => (f.split(/[\\/]/).pop() ?? f) === name)
        );
    },
    /** The Library mod backing a catalog mod, matched by `.asi` filename. */
    catalogLibMod(state) {
      return (item: CatalogMod): AsiMod | undefined => {
        const asis = catalogAsiNames(item);
        return state.asiMods.find((m) =>
          m.asiFiles.some((f) => asis.includes(f.split(/[\\/]/).pop() ?? f))
        );
      };
    },
    /**
     * The newer version string when this catalog mod offers a release newer
     * than the Library copy the user already downloaded, else null. Drives the
     * "update available" badge + Update button in the Browse view.
     */
    catalogUpdate(state) {
      return (item: CatalogMod): string | null => {
        const asis = catalogAsiNames(item);
        const lib = state.asiMods.find((m) =>
          m.asiFiles.some((f) => asis.includes(f.split(/[\\/]/).pop() ?? f))
        );
        if (
          lib &&
          item.version &&
          lib.version &&
          semverGt(item.version, lib.version)
        ) {
          return item.version;
        }
        return null;
      };
    },
    /**
     * Lifecycle state of a catalog mod, reconciled against the game folder:
     *   "deployed"   — its .asi(s) are present in the game install
     *   "enabled"    — downloaded to the Library and enabled (not yet deployed)
     *   "downloaded" — in the Library but disabled
     *   "none"       — not downloaded
     */
    /**
     * Returns the enabled catalog mod that hard-blocks `item` due to a declared
     * incompatibility (bidirectional: A blocks B if A lists B or B lists A).
     * Returns undefined if no conflict.
     */
    catalogModBlockedBy(state) {
      return (item: CatalogMod): CatalogMod | undefined => {
        const key = (c: CatalogMod) => `${c.repository}#${c.slug}`;
        const itemKey = key(item);
        return state.catalog.find((other) => {
          if (other.repository === item.repository && other.slug === item.slug) return false;
          const crossRef =
            item.incompatible.includes(key(other)) ||
            other.incompatible.includes(itemKey);
          if (!crossRef) return false;
          // Only blocks if the other mod is currently enabled in the library.
          const asis = catalogAsiNames(other);
          const lib = state.asiMods.find((m) =>
            m.asiFiles.some((f) => asis.includes(f.split(/[\\/]/).pop() ?? f))
          );
          return !!lib && state.enabled[lib.id] !== false;
        });
      };
    },

    catalogModState(state) {
      return (item: CatalogMod): "none" | "downloaded" | "enabled" | "deployed" => {
        const asis = catalogAsiNames(item);
        if (asis.length === 0) return "none";
        const deployed = new Set(
          (state.gameInfo?.deployed_asi ?? []).map((a) => a.name)
        );
        if (asis.every((a) => deployed.has(a))) return "deployed";
        const lib = state.asiMods.find((m) =>
          m.asiFiles.some((f) => asis.includes(f.split(/[\\/]/).pop() ?? f))
        );
        if (lib) return state.enabled[lib.id] !== false ? "enabled" : "downloaded";
        return "none";
      };
    },
  },

  actions: {
    /** Restore remembered settings + the saved library on app start. */
    async init() {
      this.asiTarget = localStorage.getItem(ASI_TARGET_KEY) ?? "scripts";
      this.pmcBbVersion = localStorage.getItem(PMC_BB_VERSION_KEY);
      this.crackVersion = localStorage.getItem(CRACK_VERSION_KEY);
      this.dxwrapperVersion = localStorage.getItem(DXWRAPPER_VERSION_KEY);
      this.preferredRegion = localStorage.getItem(REGION_KEY);

      // Restore the library (WAD mods, ASI plugins, enable flags, wardrobe picks).
      try {
        const raw = localStorage.getItem(LIBRARY_KEY);
        if (raw) {
          const lib = JSON.parse(raw);
          this.mods = lib.mods ?? [];
          this.asiMods = lib.asiMods ?? [];
          this.enabled = lib.enabled ?? {};
          this.wardrobe = lib.wardrobe ?? [];
          this.prebuilt = lib.prebuilt ?? [];
          this.textures = lib.textures ?? [];
        }
      } catch {
        /* ignore corrupt cache */
      }

      // Persist the library slice whenever it changes.
      this.$subscribe((_mutation, state) => {
        localStorage.setItem(
          LIBRARY_KEY,
          JSON.stringify({
            mods: state.mods,
            asiMods: state.asiMods,
            enabled: state.enabled,
            wardrobe: state.wardrobe,
            prebuilt: state.prebuilt,
            textures: state.textures,
          })
        );
      });

      await this.loadCustomSources().catch(() => {});

      const saved = localStorage.getItem(GAME_PATH_KEY);
      if (saved) {
        this.gamePath = saved;
        await this.refreshGame().catch(() => {});
      }
      if (this.mods.length) await this.refreshConflicts().catch(() => {});
    },

    setAsiTarget(target: string) {
      this.asiTarget = target;
      localStorage.setItem(ASI_TARGET_KEY, target);
    },

    async loadCustomSources() {
      this.customSources = await invoke<RepoSource[]>("get_custom_sources");
    },

    async addCustomSource(url: string) {
      const trimmed = url.trim().replace(/\.git$/, "").replace(/\/$/, "");
      // Parse https://github.com/owner/repo/tree/branch-name
      const treeMatch = trimmed.match(/^(https?:\/\/github\.com\/[^/]+\/[^/]+)\/tree\/(.+)$/);
      const repo = treeMatch ? treeMatch[1] : trimmed;
      const branch = treeMatch ? treeMatch[2] : undefined;
      const name = repo.split("/").slice(-2).join("/");
      const source: RepoSource = { name, description: "", repository: repo, ...(branch ? { branch } : {}) };
      const updated = [...this.customSources, source];
      await invoke("save_custom_sources", { sources: updated });
      this.customSources = updated;
    },

    async removeCustomSource(repository: string) {
      const norm = (u: string) =>
        u.trim().replace(/\.git$/, "").replace(/\/$/, "").toLowerCase();
      const updated = this.customSources.filter(
        (s) => norm(s.repository) !== norm(repository)
      );
      await invoke("save_custom_sources", { sources: updated });
      this.customSources = updated;
    },

    async fetchCatalog() {
      this.busy = true;
      this.error = null;
      try {
        const cat = await invoke<Catalog>("fetch_catalog");
        this.catalog = cat.mods;
        this.catalogSource = cat.source;
      } catch (e) {
        this.error = String(e);
      } finally {
        this.busy = false;
      }
    },

    /**
     * Download a catalog mod into the local Library — stages its release
     * asset(s) but leaves it DISABLED. Enabling and deploying are separate steps.
     */
    async downloadFromCatalog(item: CatalogMod): Promise<InstallResult> {
      this.busy = true;
      this.error = null;
      try {
        const res = await invoke<InstallResult>("install_catalog_mod", {
          item,
        });
        if (res.kind === "wad") {
          await this.loadModFromDir(res.mod_root);
        } else {
          const id = slugify(`${item.repo_name}-${item.slug}`);
          if (!this.asiMods.some((m) => m.id === id)) {
            this.asiMods.push({
              id,
              name: item.name,
              description: item.description,
              // Author-declared version (repository.json) so update checks compare
              // like-for-like; fall back to the release tag.
              version: item.version ?? res.version,
              modRoot: res.mod_root,
              asiFiles: res.asi_files,
            });
            // Downloaded != enabled. The user enables it explicitly.
            this.enabled[id] = false;
          }
        }
        return res;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    /** Import local `.asi` plugin file(s) as a staged ASI mod. */
    async importLocalAsi(paths: string[]) {
      if (paths.length === 0) return;
      this.busy = true;
      this.error = null;
      try {
        const res = await invoke<InstallResult>("import_local_asi", {
          paths,
          name: null,
        });
        const base = paths[0].split(/[\\/]/).pop() ?? "plugin";
        const stem = base.replace(/\.asi$/i, "");
        const id = slugify(stem);
        if (!this.asiMods.some((m) => m.id === id)) {
          this.asiMods.push({
            id,
            name: stem,
            description: "Imported locally",
            version: res.version,
            modRoot: res.mod_root,
            asiFiles: res.asi_files,
          });
          this.enabled[id] = true;
        }
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    removeAsiMod(id: string) {
      this.asiMods = this.asiMods.filter((m) => m.id !== id);
      delete this.enabled[id];
    },

    /** Adopt an already-deployed .asi into the managed Library. */
    async adoptDeployedAsi(info: DeployedAsi) {
      await this.importLocalAsi([info.abs_path]);
    },

    isAsiDeployed(mod: AsiMod): boolean {
      const deployed = this.deployedAsiNames;
      return (
        mod.asiFiles.length > 0 &&
        mod.asiFiles.every((f) => deployed.has(f.split("/").pop() ?? f))
      );
    },

    async deployAsiMod(mod: AsiMod): Promise<DeployResult> {
      if (!this.gameInfo) throw new Error("Set the game folder first");
      this.busy = true;
      this.error = null;
      try {
        const result = await invoke<DeployResult>("deploy_asi", {
          args: {
            mod_root: mod.modRoot,
            asi_files: mod.asiFiles,
            game_root: this.gameInfo.root,
            target: this.asiTarget,
          },
        });
        await this.refreshGame().catch(() => {});
        return result;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    /** Absolute paths of a mod's plugins currently present in the game folder. */
    deployedPathsForMod(mod: AsiMod): string[] {
      const wanted = new Set(mod.asiFiles.map((f) => f.split(/[\\/]/).pop() ?? f));
      return (this.gameInfo?.deployed_asi ?? [])
        .filter((d) => wanted.has(d.name))
        .map((d) => d.abs_path);
    },

    /** Move files out of the game folder (default: to the recoverable trash). */
    async trashPaths(paths: string[], permanent = false): Promise<TrashResult> {
      const res = await invoke<TrashResult>("trash_paths", { paths, permanent });
      await this.refreshGame().catch(() => {});
      return res;
    },

    /** Force-remove a single detected deployed plugin from the game folder. */
    async trashDeployedAsi(info: DeployedAsi, permanent = false) {
      this.error = null;
      try {
        await this.trashPaths([info.abs_path], permanent);
      } catch (e) {
        this.error = String(e);
        throw e;
      }
    },

    /** Undeploy a library mod: remove its plugin(s) from the game folder (trash),
     *  leaving the Library entry intact. */
    async undeployAsiMod(mod: AsiMod, permanent = false) {
      const paths = this.deployedPathsForMod(mod);
      if (paths.length === 0) return;
      this.error = null;
      try {
        await this.trashPaths(paths, permanent);
      } catch (e) {
        this.error = String(e);
        throw e;
      }
    },

    /** Undeploy (trash) and then forget a library mod entirely. */
    async forceRemoveAsiMod(mod: AsiMod, permanent = false) {
      await this.undeployAsiMod(mod, permanent).catch(() => {});
      this.removeAsiMod(mod.id);
    },

    /** Re-download a library mod from its catalog source, preserving enabled
     *  state and re-deploying if it was deployed. */
    async updateAsiMod(mod: AsiMod): Promise<void> {
      const cat = findCatalogForLib(this.catalog, mod);
      if (!cat) {
        this.error = `No catalog source found for ${mod.name}`;
        return;
      }
      this.busy = true;
      this.error = null;
      const wasDeployed = this.isAsiDeployed(mod);
      try {
        const res = await invoke<InstallResult>("install_catalog_mod", {
          item: cat,
        });
        const lib = this.asiMods.find((m) => m.id === mod.id);
        if (lib) {
          lib.version = cat.version ?? res.version;
          lib.modRoot = res.mod_root;
          lib.asiFiles = res.asi_files;
          if (cat.description) lib.description = cat.description;
          if (wasDeployed) await this.deployAsiMod(lib);
        }
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    /**
     * Check for a newer modkit release. Prefers the Tauri updater (signed
     * manifest, in-place install); falls back to the GitHub release lookup for
     * install forms the updater can't replace (portable exe, deb/rpm/flatpak)
     * or when no update manifest is published yet.
     */
    async checkModkitUpdate() {
      let current = "";
      try {
        current = await getVersion();
        // Show the real version immediately, even if the release lookup fails.
        this.modkitUpdate = {
          current,
          latest: current,
          url: `${MODKIT_REPO}/releases`,
          available: false,
          canInstall: false,
        };
      } catch {
        /* version unavailable (non-Tauri context) */
      }
      try {
        if (await invoke<boolean>("updater_supported")) {
          const upd = await checkUpdate();
          if (upd) {
            pendingUpdate = upd;
            this.modkitUpdate = {
              current,
              latest: upd.version,
              url: `${MODKIT_REPO}/releases`,
              available: true,
              canInstall: true,
            };
            return;
          }
          // Updater reachable and reports up to date — trust it.
          return;
        }
      } catch {
        /* no manifest yet / offline / dev build — fall back to the API check */
      }
      try {
        const rel = await invoke<ReleaseInfo>("latest_release", {
          repo: MODKIT_REPO,
        });
        this.modkitUpdate = {
          current,
          latest: rel.tag,
          url: rel.url,
          available: !!current && semverGt(rel.tag, current),
          canInstall: false,
        };
      } catch {
        /* offline or no releases yet — keep the current-version-only state */
      }
    },

    /**
     * Download and install the update found by {@link checkModkitUpdate},
     * then relaunch into the new version. Only valid when
     * `modkitUpdate.canInstall` is true.
     */
    async installModkitUpdate() {
      if (!pendingUpdate || this.updateInstalling) return;
      this.updateInstalling = true;
      this.updateProgress = null;
      this.error = null;
      let total = 0;
      let received = 0;
      try {
        await pendingUpdate.downloadAndInstall((ev) => {
          if (ev.event === "Started") {
            total = ev.data.contentLength ?? 0;
            this.updateProgress = total ? 0 : null;
          } else if (ev.event === "Progress") {
            received += ev.data.chunkLength;
            if (total) {
              this.updateProgress = Math.min(
                100,
                Math.round((received / total) * 100)
              );
            }
          } else if (ev.event === "Finished") {
            this.updateProgress = 100;
          }
        });
        await relaunch();
      } catch (e) {
        this.error = `Update failed: ${e}`;
        this.updateInstalling = false;
        this.updateProgress = null;
      }
    },

    /**
     * Check the GitHub releases of modkit's core components (the pmc_bb.dll ASI
     * loader and the apply_crack tool) for newer versions than the ones modkit
     * last installed. Mirrors {@link checkModkitUpdate}; results land in
     * `componentUpdates` keyed by component id. Best-effort — offline / no-release
     * lookups are ignored so any prior result is preserved.
     */
    async checkComponentUpdates() {
      const checks: Array<{
        key: string;
        name: string;
        repo: string;
        current: string | null;
      }> = [
        {
          key: "pmc_bb",
          name: "pmc_bb.dll (ASI loader)",
          repo: PMC_BB_REPO,
          current: this.pmcBbVersion,
        },
        {
          key: "apply_crack",
          name: "apply_crack (SecuROM bypass)",
          repo: CRACK_REPO,
          current: this.crackVersion,
        },
        {
          key: "dxwrapper",
          name: "dxwrapper",
          repo: DXWRAPPER_REPO,
          current: this.dxwrapperVersion,
        },
      ];
      for (const { key, name, repo, current } of checks) {
        try {
          const rel = await invoke<ReleaseInfo>("latest_release", { repo });
          this.componentUpdates[key] = {
            name,
            current,
            latest: rel.tag,
            url: rel.url,
            available: !!current && semverGt(rel.tag, current),
          };
        } catch {
          /* offline or no releases yet — keep any prior result */
        }
      }
    },

    /**
     * Refresh the Workshop toolset status. `checkRemote` also asks GitHub for
     * the latest release tag; skip it for a cheap local-only refresh.
     *
     * Best-effort like the other update checks — an offline lookup still returns
     * what is installed, so the page renders either way.
     */
    async refreshToolset(checkRemote = true) {
      try {
        this.toolset = await invoke<ToolsetStatus>("toolset_status", {
          checkRemote,
        });
      } catch {
        /* toolset dir unreadable — leave any prior status in place */
      }
    },

    /**
     * Install the named tools at the latest release, bringing everything already
     * installed up to that same release with them. Pass nothing to just update.
     *
     * The union is enforced in Rust: the toolset ships as one release, so modkit
     * never leaves a mix of tags on disk.
     */
    async installTools(names: string[] = []) {
      if (this.toolsetProgress) return;
      this.error = null;
      this.toolsetProgress = { tool: "", label: "", done: 0, total: 0 };
      try {
        this.toolset = await invoke<ToolsetStatus>("install_tools", { names });
      } catch (e) {
        this.error = `Toolset install failed: ${e}`;
      } finally {
        this.toolsetProgress = null;
      }
    },

    /**
     * Open the platform's default terminal (PowerShell on Windows, Terminal on
     * macOS, the desktop's emulator on Linux) in the user's home folder, with
     * the installed CLI tools on PATH.
     */
    async openToolShell() {
      this.error = null;
      try {
        await invoke("open_tool_shell");
      } catch (e) {
        this.error = String(e);
      }
    },

    /**
     * Launch one of the windowed tools (Workshop, native game), handing it the
     * game install modkit already detected so the user is not asked for a path
     * modkit knows.
     */
    async launchTool(name: string) {
      this.error = null;
      try {
        // The Tools page can be the first thing opened in a session, so the
        // saves listing may not have been fetched yet. Without this the game
        // falls back to its own default and misses a custom saves folder.
        if (!this.savesInfo) await this.refreshSaves().catch(() => {});
        await invoke("launch_tool", {
          name,
          gameDir: this.gamePath,
          savesDir: this.savesInfo?.dir ?? null,
        });
        // Reflect it immediately rather than waiting up to a poll interval for
        // the button to catch up.
        if (!this.runningTools.includes(name)) this.runningTools.push(name);
      } catch (e) {
        this.error = String(e);
      }
    },

    /** Stop a tool modkit started. */
    async stopTool(name: string) {
      this.error = null;
      try {
        await invoke("stop_tool", { name });
        this.runningTools = this.runningTools.filter((n) => n !== name);
      } catch (e) {
        this.error = String(e);
      }
    },

    /**
     * Refresh which tools are running, mirroring `refreshRunning` for the game.
     * Crashes come back here too — a tool that dies on startup would otherwise
     * just flip its button back to "Open" with no explanation.
     */
    async pollTools() {
      try {
        const res = await invoke<ToolsRunning>("poll_tools");
        this.runningTools = res.running;
        if (res.failures.length) {
          this.error = res.failures
            .map((f) => `${f.label} ${f.message}`)
            .join("\n\n");
        }
      } catch {
        /* backend not reachable — leave the last known state alone */
      }
    },

    /** Remove one installed tool (the engine-backed apps are the large ones). */
    async uninstallTool(name: string) {
      this.error = null;
      try {
        this.toolset = await invoke<ToolsetStatus>("uninstall_tool", { name });
      } catch (e) {
        this.error = String(e);
      }
    },

    /** Refresh the live saves listing and the stored snapshot list. */
    async refreshSaves() {
      this.savesInfo = await invoke<SavesInfo>("list_saves", { prefix: null });
      this.saveBackups = await invoke<SaveBackupInfo[]>("list_save_backups");
    },

    /** Set (or clear, with null) where modkit looks for the SaveGames folder. */
    async setSavesDir(dir: string | null) {
      this.error = null;
      try {
        await invoke("set_saves_dir", { dir });
        await this.refreshSaves();
      } catch (e) {
        this.error = String(e);
        throw e;
      }
    },

    /** Snapshot the current saves now. Returns what happened (may be a skip). */
    async backupSavesNow(): Promise<BackupResult> {
      this.savesBusy = true;
      this.error = null;
      try {
        const res = await invoke<BackupResult>("backup_saves", {
          prefix: null,
          reason: "manual",
        });
        await this.refreshSaves();
        return res;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.savesBusy = false;
      }
    },

    /**
     * Copy a snapshot's saves back over the live SaveGames folder. The backend
     * snapshots the current state first, so a restore is always undoable.
     */
    async restoreSaveBackup(id: string): Promise<RestoreResult> {
      this.savesBusy = true;
      this.error = null;
      try {
        const res = await invoke<RestoreResult>("restore_save_backup", {
          id,
          prefix: null,
        });
        await this.refreshSaves();
        return res;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.savesBusy = false;
      }
    },

    /** Permanently delete one stored snapshot. */
    async deleteSaveBackup(id: string) {
      this.savesBusy = true;
      this.error = null;
      try {
        await invoke("delete_save_backup", { id });
        await this.refreshSaves();
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.savesBusy = false;
      }
    },

    async setGameFolder(path: string) {
      this.gamePath = path;
      localStorage.setItem(GAME_PATH_KEY, path);
      await this.refreshGame();
    },


    async refreshGame() {
      if (!this.gamePath) return;
      this.error = null;
      try {
        this.gameInfo = await invoke<GameInfo>("detect_game", {
          path: this.gamePath,
        });
      } catch (e) {
        this.gameInfo = null;
        this.error = String(e);
        throw e;
      }
      // Probe the host runtime + matchmaking region + license alongside
      // detection (non-fatal if any fails).
      void this.checkVcRedist();
      void this.checkRegion();
      void this.checkLicense();
    },

    /** Detect a SecuROM activation so Setup can offer the dxwrapper path. */
    async checkLicense() {
      try {
        this.license = await invoke<LicenseStatus>("detect_license");
      } catch {
        /* leave any prior result in place */
      }
    },

    /** Check whether the host has the 32-bit VC++ 2008 runtime the game needs. */
    async checkVcRedist() {
      try {
        this.vcRedist = await invoke<VcRedistStatus>("check_vcredist");
      } catch {
        /* leave any prior result in place */
      }
    },

    /** Download & run the Microsoft-signed VC++ 2008 redistributable. */
    async installVcRedist(): Promise<InstallVcRedistResult> {
      this.busy = true;
      this.error = null;
      try {
        const res = await invoke<InstallVcRedistResult>("install_vcredist");
        await this.checkVcRedist();
        return res;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    /** Read the matchmaking-relevant EA Games registry key for this install. */
    async checkRegion() {
      if (!this.gameInfo) return;
      try {
        this.region = await invoke<RegionStatus>("read_region", {
          gameRoot: this.gameInfo.root,
          preferredRegion: this.preferredRegion,
        });
      } catch {
        /* leave any prior result in place */
      }
    },

    /**
     * Remember which region's matchmaking pool the user wants (persisted) and
     * re-judge the registry against it. Doesn't write the registry — that's
     * `normalizeRegion`.
     */
    setPreferredRegion(region: string) {
      this.preferredRegion = region;
      localStorage.setItem(REGION_KEY, region);
      void this.checkRegion();
    },

    /**
     * Write the EA Games install key with the user's selected `Region` (the
     * pool default if they never picked one) so this install matchmakes with
     * everyone sharing that region. Elevated (UAC prompt).
     */
    async normalizeRegion(region?: string): Promise<NormalizeRegionResult> {
      if (!this.gameInfo) throw new Error("Set the game folder first");
      this.busy = true;
      this.error = null;
      try {
        const res = await invoke<NormalizeRegionResult>("normalize_region", {
          gameRoot: this.gameInfo.root,
          region: region ?? this.preferredRegion,
        });
        await this.checkRegion();
        return res;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    /** List which languages' content (`.wad`/`.pws`) the install carries. */
    async scanLanguages(): Promise<LanguageStatus> {
      if (!this.gameInfo) throw new Error("Set the game folder first");
      this.error = null;
      return await invoke<LanguageStatus>("scan_languages", {
        gameRoot: this.gameInfo.root,
      });
    },

    /** Keep one language and move the others' files to the recoverable trash. */
    async setLanguage(language: string): Promise<SetLanguageResult> {
      if (!this.gameInfo) throw new Error("Set the game folder first");
      this.busy = true;
      this.error = null;
      try {
        const res = await invoke<SetLanguageResult>("set_language", {
          gameRoot: this.gameInfo.root,
          language,
        });
        await this.refreshGame().catch(() => {});
        return res;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    /** Verify the install against a known-good manifest (Steam-style). Pass a
     *  local manifest path to test before publishing; omit it to fetch the
     *  published manifest for the detected version. */
    async verifyGame(manifestPath?: string): Promise<VerifyReport> {
      if (!this.gameInfo) throw new Error("Set the game folder first");
      this.error = null;
      return await invoke<VerifyReport>("verify_game", {
        gameRoot: this.gameInfo.root,
        manifestPath: manifestPath ?? null,
      });
    },

    /**
     * Bundle everything needed to diagnose this install into a dated `.zip` at
     * `destPath`: the game logs, an inventory of installed mods, the versions of
     * every moving part, and a fresh file-integrity check. Backend emits
     * `debug-status` phase text while it works.
     */
    async buildDebugZip(destPath: string): Promise<DebugZipResult> {
      if (!this.gameInfo) throw new Error("Set the game folder first");
      this.error = null;
      let modkitVersion = "unknown";
      try {
        modkitVersion = await getVersion();
      } catch {
        /* non-Tauri context — leave "unknown" */
      }
      const meta = {
        generatedAt: new Date().toISOString(),
        modkitVersion,
        game: this.gameInfo,
        pmcBbVersion: this.pmcBbVersion,
        crackVersion: this.crackVersion,
        componentUpdates: this.componentUpdates,
        vcRedist: this.vcRedist,
        region: this.region,
        catalogSource: this.catalogSource,
        wadMods: this.mods.map((m) => ({
          id: m.id,
          name: m.manifest.name,
          version: m.manifest.version,
          enabled: this.isEnabled(m.id),
          assetCount: m.assets.length,
        })),
        asiMods: this.asiMods.map((m) => ({
          id: m.id,
          name: m.name,
          version: m.version,
          enabled: this.isEnabled(m.id),
          deployed: this.isAsiDeployed(m),
        })),
        deployedAsi: this.gameInfo.deployed_asi,
        deployedPatches: this.gameInfo.deployed_patches,
      };
      return await invoke<DebugZipResult>("build_debug_zip", {
        gameRoot: this.gameInfo.root,
        destPath,
        meta,
      });
    },

    /** Maintainer tool: hash a clean install into a reference manifest. */
    async generateManifest(): Promise<GenerateManifestResult> {
      if (!this.gameInfo) throw new Error("Set the game folder first");
      this.error = null;
      return await invoke<GenerateManifestResult>("generate_manifest", {
        gameRoot: this.gameInfo.root,
        version: this.gameInfo.version,
        variant: this.gameInfo.variant,
      });
    },

    async loadModFromDir(path: string) {
      this.busy = true;
      this.error = null;
      try {
        const mod = await invoke<LoadedMod>("load_mod", { path });
        if (this.mods.some((m) => m.id === mod.id)) {
          throw new Error(`Mod "${mod.id}" is already loaded`);
        }
        this.mods.push(mod);
        this.enabled[mod.id] = true;
        await this.refreshConflicts();
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    removeMod(id: string) {
      this.mods = this.mods.filter((m) => m.id !== id);
      delete this.enabled[id];
      for (const [hash, res] of Object.entries(this.resolutions)) {
        if ("modId" in res && res.modId === id) delete this.resolutions[hash];
      }
      void this.refreshConflicts();
    },

    toggleMod(id: string) {
      this.enabled[id] = this.enabled[id] === false;
      void this.refreshConflicts();
    },

    /** Set a mod's enabled state explicitly (intent; does not deploy). */
    setModEnabled(id: string, value: boolean) {
      this.enabled[id] = value;
    },

    /**
     * Move a mod earlier ("up") or later ("down") in the load order.
     *
     * Later = wins. The engine mounts vz.wad then vz-patch.wad and takes the LAST
     * match for an asset, so the mod at the bottom of the list overrides the ones
     * above it. Don't call this "priority" in the UI — say "later mods override
     * earlier ones", which is the same convention MO2/Vortex users already know.
     */
    moveMod(id: string, dir: "up" | "down") {
      const i = this.mods.findIndex((m) => m.id === id);
      if (i < 0) return;
      const j = dir === "up" ? i - 1 : i + 1;
      if (j < 0 || j >= this.mods.length) return;
      const next = this.mods.slice();
      [next[i], next[j]] = [next[j], next[i]];
      this.mods = next;
    },

    async refreshConflicts() {
      this.conflictGraph = await invoke<ConflictGraph>("build_conflict_graph", {
        mods: this.enabledMods,
      });
    },

    setResolution(assetHash: number, res: Resolution) {
      this.resolutions[String(assetHash)] = res;
    },

    /**
     * Apply explicit conflict resolutions over the enabled mods, in load order.
     *
     * Unresolved overlaps are decided by the backend, which is LAST-wins (the mod
     * lowest in the list overrides those above it — the engine's own rule). Explicit
     * `priority` / `exclude` resolutions override that per asset.
     */
    resolvedMods(): LoadedMod[] {
      return this.enabledMods.map((m) => {
        const assets = m.assets.filter((a) => {
          const res = this.resolutions[String(a.asset_hash)];
          if (!res) return true;
          if (res.kind === "exclude") return false;
          if (res.kind === "priority") return res.modId === m.id;
          return true;
        });
        return { ...m, assets };
      });
    },

    /**
     * Build `vz-patch.wad` from the load order into a staging directory.
     *
     * This does NOT touch the game — `deployPatchWad` installs it. Keeping build and
     * install separate is deliberate: the old flow defaulted its output straight into
     * the game's data dir and overwrote the live `vz-patch.wad` with no backup.
     */
    async assemble(opts: { outputDir?: string } = {}) {
      this.busy = true;
      this.error = null;
      this.buildResult = null;
      try {
        this.buildResult = await invoke<BuildResult>("assemble_patch_wad", {
          options: this.buildOptions(opts.outputDir ?? null),
        });
        return this.buildResult;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    /** Everything that goes into a build, in load order. Shared by build and preview so
     *  the dry-run can never disagree with what actually gets assembled. */
    buildOptions(outputDir: string | null = null) {
      return {
        mods: this.resolvedMods(),
        output_dir: outputDir,
        game_path: this.gamePath,
        wardrobe: this.wardrobe,
        prebuilt: this.prebuilt,
        textures: this.textures,
      };
    },

    /** Look a texture up in the user's own vz.wad (size, format, whether we can swap it). */
    async inspectTexture(name: string): Promise<TextureTarget> {
      if (!this.gamePath) throw new Error("Set the game folder first");
      return await invoke<TextureTarget>("inspect_texture", {
        gamePath: this.gamePath,
        name,
      });
    },

    /**
     * Full detail for one texture, including which models use it.
     *
     * The "used by" relation isn't stored anywhere in the game — it's inverted out of every
     * model's material slots. The FIRST call on an install builds that index and takes
     * ~10 seconds; every call after reads it from cache. Show a spinner on the first one.
     */
    async textureDetails(name: string): Promise<TextureDetails> {
      if (!this.gamePath) throw new Error("Set the game folder first");
      return await invoke<TextureDetails>("texture_details", {
        gamePath: this.gamePath,
        name,
      });
    },

    /**
     * Geometry for one model, with every draw group flagged if its material samples
     * `texture` — that flag is what the 3D viewer lights up.
     */
    async modelGeometry(
      model: string,
      texture: string,
      tier: number | null = null,
    ): Promise<ModelGeometry> {
      if (!this.gamePath) throw new Error("Set the game folder first");
      return await invoke<ModelGeometry>("model_geometry", {
        gamePath: this.gamePath,
        model,
        texture,
        tier,
      });
    },

    /**
     * Every state/LOD variant of a model, flagged with whether the texture appears in it.
     *
     * This is what turns "this texture isn't visible" from a dead end into an answer: it
     * says which states DO show it, so the user can switch to one.
     */
    async modelVariants(model: string, texture: string): Promise<ModelVariant[]> {
      if (!this.gamePath) throw new Error("Set the game folder first");
      return await invoke<ModelVariant[]>("model_variants", {
        gamePath: this.gamePath,
        model,
        texture,
      });
    },

    /** Every part, in every model, that paints this texture. */
    async textureParts(texture: string): Promise<TexturePart[]> {
      if (!this.gamePath) throw new Error("Set the game folder first");
      return await invoke<TexturePart[]>("texture_parts", {
        gamePath: this.gamePath,
        texture,
      });
    },

    /** Write a texture out as a PNG the user can edit. */
    async exportTexture(name: string, dest: string): Promise<TextureExport> {
      if (!this.gamePath) throw new Error("Set the game folder first");
      return await invoke<TextureExport>("export_texture", {
        gamePath: this.gamePath,
        name,
        dest,
      });
    },

    /** Every nameable texture in this install. Reads only the ASET table, so it's fast. */
    async loadTextureCatalog() {
      if (!this.gamePath) return;
      if (this.textureCatalog.length) return; // already built for this install
      this.textureCatalog = await invoke<TextureEntry[]>("list_textures", {
        gamePath: this.gamePath,
      });
    },

    /**
     * Decode thumbnails for the rows about to be shown.
     *
     * Batched on purpose: each one costs a block decompression, so the view asks for the
     * page it needs rather than all ~13,000.
     */
    async loadTexturePreviews(names: string[]): Promise<TexturePreview[]> {
      if (!this.gamePath || !names.length) return [];
      return await invoke<TexturePreview[]>("texture_previews", {
        gamePath: this.gamePath,
        names,
        maxSize: 128,
      });
    },

    addTextureSwap(t: TextureSwap) {
      this.textures = [...this.textures.filter((x) => x.name !== t.name), t];
    },

    removeTextureSwap(name: string) {
      this.textures = this.textures.filter((t) => t.name !== name);
    },

    /**
     * Dry-run the load order: what applies, what gets overridden, what can't be
     * resolved. Throws the structured conflict list if two mods overlap partially.
     */
    async previewConflicts(): Promise<BuildResult> {
      return await invoke<BuildResult>("preview_conflicts", {
        options: this.buildOptions(),
      });
    },

    /** Install a built WAD into the game, snapshotting whatever it replaces. */
    async deployPatchWad(wadPath: string): Promise<DeployWadResult> {
      if (!this.gameInfo?.data_dir) throw new Error("Set the game folder first");
      this.busy = true;
      this.error = null;
      try {
        const res = await invoke<DeployWadResult>("deploy_patch_wad", {
          args: { wad_path: wadPath, data_dir: this.gameInfo.data_dir },
        });
        await this.refreshGame();
        await this.loadWadBackups();
        return res;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    async loadWadBackups() {
      this.wadBackups = await invoke<WadBackup[]>("list_patch_wad_backups");
    },

    /**
     * Load the wearable models THIS install actually has.
     *
     * The backend checks each candidate against the user's own vz.wad ASET table, so the
     * list can only contain models that will really load. A DLC skin the player doesn't
     * own simply isn't returned.
     */
    async loadWardrobeModels() {
      if (!this.gamePath) return;
      this.wardrobeModels = await invoke<WardrobeModel[]>("list_wardrobe_models", {
        gamePath: this.gamePath,
      });
    },

    /**
     * Import a community-made `vz-patch.wad` into the load order.
     *
     * This is what makes two prebuilt mods installable at once — the game itself only ever
     * loads one patch WAD, so modkit merges them into one.
     */
    async importPatchWad(path: string): Promise<PrebuiltWad> {
      this.busy = true;
      this.error = null;
      try {
        const info = await invoke<PrebuiltWad>("inspect_patch_wad", { path });
        if (!this.prebuilt.some((p) => p.path === info.path)) {
          this.prebuilt = [...this.prebuilt, info];
        }
        return info;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    removePrebuilt(id: string) {
      this.prebuilt = this.prebuilt.filter((p) => p.id !== id);
    },

    /** Move an imported WAD earlier/later. Later = overrides the ones above it. */
    movePrebuilt(id: string, dir: "up" | "down") {
      const i = this.prebuilt.findIndex((p) => p.id === id);
      if (i < 0) return;
      const j = dir === "up" ? i - 1 : i + 1;
      if (j < 0 || j >= this.prebuilt.length) return;
      const next = this.prebuilt.slice();
      [next[i], next[j]] = [next[j], next[i]];
      this.prebuilt = next;
    },

    addWardrobeOutfit(o: WardrobeOutfit) {
      if (this.wardrobe.some((x) => x.hero === o.hero && x.model === o.model)) return;
      this.wardrobe = [...this.wardrobe, o];
    },

    removeWardrobeOutfit(hero: string, model: string) {
      this.wardrobe = this.wardrobe.filter(
        (o) => !(o.hero === hero && o.model === model),
      );
    },

    /**
     * Restore a previous WAD, or (with no `file`) remove the patch entirely and go
     * back to the stock game — always a safe state.
     */
    async restorePatchWad(file: string | null): Promise<DeployWadResult> {
      if (!this.gameInfo?.data_dir) throw new Error("Set the game folder first");
      this.busy = true;
      this.error = null;
      try {
        const res = await invoke<DeployWadResult>("restore_patch_wad", {
          args: { file, data_dir: this.gameInfo.data_dir },
        });
        await this.refreshGame();
        await this.loadWadBackups();
        return res;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    /** Download the latest pmc_bb.dll (ASI loader) into the game folder. */
    async installPmcBb(): Promise<InstallDllResult> {
      if (!this.gameInfo) throw new Error("Set the game folder first");
      this.busy = true;
      this.error = null;
      try {
        const res = await invoke<InstallDllResult>("install_pmc_bb", {
          gameRoot: this.gameInfo.root,
        });
        // Remember what we just installed so future release checks compare
        // like-for-like (this also clears any stale "update available" flag).
        this.pmcBbVersion = res.version;
        localStorage.setItem(PMC_BB_VERSION_KEY, res.version);
        await this.refreshGame().catch(() => {});
        void this.checkComponentUpdates();
        return res;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    /**
     * Set up the licensed (non-destructive) mod path: install the logging-only
     * pmc_bb.dll and dxwrapper, leaving the exe untouched. One action so the
     * Setup page can offer it as a single step. Order matters only cosmetically
     * (both land before launch); pmc_bb first so the loader it needs exists.
     */
    async setupDxwrapper(): Promise<DxwrapperResult> {
      if (!this.gameInfo) throw new Error("Set the game folder first");
      this.busy = true;
      this.error = null;
      try {
        // Install dxwrapper FIRST, so it (and its tracked version) land even if the
        // logging pmc_bb download isn't available yet.
        const res = await invoke<DxwrapperResult>("install_dxwrapper", {
          gameRoot: this.gameInfo.root,
        });
        if (res.version) {
          this.dxwrapperVersion = res.version;
          localStorage.setItem(DXWRAPPER_VERSION_KEY, res.version);
        }
        // Then the logging bridge (installed as pmc_bb.dll). If that download fails
        // (e.g. the release hasn't published pmc_bb_log.dll yet) but a pmc_bb.dll is
        // already present, keep going — only a total absence of the loader is fatal.
        try {
          const log = await invoke<InstallDllResult>("install_pmc_bb_log", {
            gameRoot: this.gameInfo.root,
          });
          this.pmcBbVersion = log.version;
          localStorage.setItem(PMC_BB_VERSION_KEY, log.version);
        } catch (e) {
          await this.refreshGame().catch(() => {});
          if (!this.gameInfo?.has_pmc_bb) throw e;
          this.error = `dxwrapper installed, but the logging pmc_bb.dll couldn't be downloaded (${e}). Keeping the pmc_bb.dll already present.`;
        }
        await this.refreshGame().catch(() => {});
        return res;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    /** Crack the exe via apply_crack (auto-updates v1.0 → v1.1, then bypasses). */
    async crackGame(opts: { outputPath: string | null }): Promise<CrackResult> {
      if (!this.gameInfo) throw new Error("Set the game folder first");
      this.busy = true;
      this.error = null;
      try {
        const res = await invoke<CrackResult>("crack_game", {
          exePath: this.gameInfo.exe_path,
          outputPath: opts.outputPath,
        });
        // Remember the apply_crack build we ran so a later release shows as an
        // available update.
        if (res.tool_version) {
          this.crackVersion = res.tool_version;
          localStorage.setItem(CRACK_VERSION_KEY, res.tool_version);
          void this.checkComponentUpdates();
        }
        await this.refreshGame().catch(() => {});
        return res;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    /**
     * Update the exe to the official v1.1 WITHOUT cracking (apply_crack
     * --update-only): installs the retail, still-DRM'd v1.1 in place, backing the
     * original up to BACKUP/. For a licensed v1.0 copy — activation carries over.
     */
    async updateGame(): Promise<CrackResult> {
      if (!this.gameInfo) throw new Error("Set the game folder first");
      this.busy = true;
      this.error = null;
      try {
        const res = await invoke<CrackResult>("update_game", {
          exePath: this.gameInfo.exe_path,
        });
        if (res.tool_version) {
          this.crackVersion = res.tool_version;
          localStorage.setItem(CRACK_VERSION_KEY, res.tool_version);
          void this.checkComponentUpdates();
        }
        await this.refreshGame().catch(() => {});
        return res;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    /** Launch the game exe with the install folder as working directory. */
    /** Find pmc_blackbox.log near the install, then analyze it with loadprobe. */
    async locateLog(): Promise<string | null> {
      if (!this.gameInfo) return null;
      return await invoke<string | null>("locate_log", {
        gameRoot: this.gameInfo.root,
      });
    },

    async analyzeLog(path: string): Promise<LogReport> {
      this.busy = true;
      this.error = null;
      try {
        return await invoke<LogReport>("analyze_log", { path });
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },

    /** Resolve Proton/runtime paths (autodiscovery + overrides) for display. */
    async discoverRuntime(overrides: RuntimeOverrides | null = null) {
      return await invoke<RuntimeInfo>("discover_runtime", { overrides });
    },

    async launchGame(
      overrides: RuntimeOverrides | null = null,
      verboseLog = false,
    ) {
      if (!this.gameInfo) throw new Error("Set the game folder first");
      this.error = null;
      try {
        await invoke("launch_game", {
          exePath: this.gameInfo.exe_path,
          gameRoot: this.gameInfo.root,
          overrides,
          verboseLog,
        });
        this.gameRunning = true;
      } catch (e) {
        this.error = String(e);
        // Reconcile with reality (e.g. "already running" means it IS running).
        await this.refreshRunning();
        throw e;
      }
    },

    /** Stop the instance modkit launched. */
    async stopGame() {
      this.error = null;
      try {
        await invoke("stop_game");
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        await this.refreshRunning();
      }
    },

    /** Poll whether our launched instance is still alive. */
    async refreshRunning() {
      try {
        this.gameRunning = await invoke<boolean>("is_game_running");
      } catch {
        this.gameRunning = false;
      }
    },

    async fetchSimulator(): Promise<string> {
      return await invoke<string>("fetch_wad_simulator");
    },

    async validate(wadPath: string, simulatorPath: string | null) {
      this.busy = true;
      this.error = null;
      try {
        this.validation = await invoke<ValidationResult>("validate_wad", {
          wadPath,
          simulatorPath,
        });
        return this.validation;
      } catch (e) {
        this.error = String(e);
        throw e;
      } finally {
        this.busy = false;
      }
    },
  },
});
