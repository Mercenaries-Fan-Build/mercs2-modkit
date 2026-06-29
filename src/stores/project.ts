import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import type {
  AsiMod,
  BuildResult,
  CatalogMod,
  Catalog,
  RepoSource,
  ComponentUpdate,
  ConflictGraph,
  CrackResult,
  DeployedAsi,
  DeployResult,
  GameInfo,
  InstallDllResult,
  InstallResult,
  LoadedMod,
  LogReport,
  ModkitUpdate,
  ReleaseInfo,
  Resolution,
  RuntimeInfo,
  RuntimeOverrides,
  TrashResult,
  ValidationResult,
  VcRedistStatus,
  InstallVcRedistResult,
  VerifyReport,
  GenerateManifestResult,
} from "../types";

const GAME_PATH_KEY = "mercs2-modkit:gamePath";
const ASI_TARGET_KEY = "mercs2-modkit:asiTarget";
const LIBRARY_KEY = "mercs2-modkit:library";
// Versions of the core components modkit last installed, remembered so a later
// release of either can be flagged as an available update.
const PMC_BB_VERSION_KEY = "mercs2-modkit:pmcBbVersion";
const CRACK_VERSION_KEY = "mercs2-modkit:crackVersion";

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
/** Repos publishing the core components modkit installs (release-checked too). */
const PMC_BB_REPO = "https://github.com/Mercenaries-Fan-Build/pmc-blackbox";
const CRACK_REPO = "https://github.com/Mercenaries-Fan-Build/mercs2-securom-bypass";

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
  // Versions of the core components modkit last installed (null = unknown).
  pmcBbVersion: string | null;
  crackVersion: string | null;
  // Release-update status per core component, keyed "pmc_bb" / "apply_crack".
  componentUpdates: Record<string, ComponentUpdate>;
  // Host's 32-bit VC++ 2008 runtime status (null = not yet checked).
  vcRedist: VcRedistStatus | null;
  // Settings
  asiTarget: string; // ".", "scripts", "plugins", "update"
  // Conflicts & build
  conflictGraph: ConflictGraph | null;
  resolutions: Record<string, Resolution>;
  buildResult: BuildResult | null;
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
    pmcBbVersion: null,
    crackVersion: null,
    componentUpdates: {},
    vcRedist: null,
    asiTarget: "scripts",
    conflictGraph: null,
    resolutions: {},
    buildResult: null,
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
    /** Fully prepared for modding: v1.1, cracked, with the ASI loader installed. */
    gameFullySetUp(state): boolean {
      const g = state.gameInfo;
      return (
        !!g && g.version === "v1.1" && g.variant === "cracked" && g.has_pmc_bb
      );
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

      // Restore the library (WAD mods, ASI plugins, enable flags).
      try {
        const raw = localStorage.getItem(LIBRARY_KEY);
        if (raw) {
          const lib = JSON.parse(raw);
          this.mods = lib.mods ?? [];
          this.asiMods = lib.asiMods ?? [];
          this.enabled = lib.enabled ?? {};
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

    /** Check modkit's own GitHub releases for a newer version. */
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
        };
      } catch {
        /* version unavailable (non-Tauri context) */
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
        };
      } catch {
        /* offline or no releases yet — keep the current-version-only state */
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
      // Probe the host runtime alongside detection (non-fatal if it fails).
      void this.checkVcRedist();
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

    /** Move a mod up (higher priority) or down in the load order. */
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
     * Apply conflict resolutions over the enabled mods (in load order). The
     * backend keeps the first occurrence of each hash, so top-of-list wins
     * unresolved conflicts; explicit resolutions override that.
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

    async assemble(opts: {
      outputDir: string;
      splitByPatch: boolean;
      mergeInto: string | null;
    }) {
      this.busy = true;
      this.error = null;
      this.buildResult = null;
      try {
        this.buildResult = await invoke<BuildResult>("assemble_patch_wad", {
          options: {
            mods: this.resolvedMods(),
            excluded_assets: [],
            output_dir: opts.outputDir,
            split_by_patch: opts.splitByPatch,
            merge_into: opts.mergeInto,
          },
        });
        return this.buildResult;
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

    /** Crack the exe (and optionally update v1.0 → v1.1) via apply_crack. */
    async crackGame(opts: {
      updateToV11: boolean;
      outputPath: string | null;
    }): Promise<CrackResult> {
      if (!this.gameInfo) throw new Error("Set the game folder first");
      this.busy = true;
      this.error = null;
      try {
        const res = await invoke<CrackResult>("crack_game", {
          exePath: this.gameInfo.exe_path,
          outputPath: opts.outputPath,
          updateToV11: opts.updateToV11,
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
