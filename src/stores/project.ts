import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import type {
  AsiMod,
  BuildResult,
  CatalogEntry,
  Catalog,
  ConflictGraph,
  CrackResult,
  DeployedAsi,
  DeployResult,
  GameInfo,
  InstallDllResult,
  InstallResult,
  LoadedMod,
  LogReport,
  Resolution,
  ValidationResult,
} from "../types";

const GAME_PATH_KEY = "mercs2-modkit:gamePath";
const ASI_TARGET_KEY = "mercs2-modkit:asiTarget";
const LIBRARY_KEY = "mercs2-modkit:library";

function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

interface ProjectState {
  // Base game
  gamePath: string | null;
  gameInfo: GameInfo | null;
  // WAD-asset mods — array order is the load order (top wins conflicts).
  mods: LoadedMod[];
  // ASI-plugin mods (deployed into the game's ASI loader folder).
  asiMods: AsiMod[];
  /** mod id -> enabled (defaults to true). Shared across both mod kinds. */
  enabled: Record<string, boolean>;
  // Curated catalog
  catalog: CatalogEntry[];
  catalogSource: string | null;
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
    mods: [],
    asiMods: [],
    enabled: {},
    catalog: [],
    catalogSource: null,
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
    /** Whether a deployed plugin filename is already managed in the Library. */
    isAsiManaged(state) {
      return (name: string): boolean =>
        state.asiMods.some((m) =>
          m.asiFiles.some((f) => (f.split(/[\\/]/).pop() ?? f) === name)
        );
    },
  },

  actions: {
    /** Restore remembered settings + the saved library on app start. */
    async init() {
      this.asiTarget = localStorage.getItem(ASI_TARGET_KEY) ?? "scripts";

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

    async fetchCatalog() {
      this.busy = true;
      this.error = null;
      try {
        const cat = await invoke<Catalog>("fetch_catalog");
        this.catalog = cat.entries;
        this.catalogSource = cat.source;
      } catch (e) {
        this.error = String(e);
      } finally {
        this.busy = false;
      }
    },

    /** Install a catalog entry: stage its release, then register it by kind. */
    async installFromCatalog(entry: CatalogEntry): Promise<InstallResult> {
      this.busy = true;
      this.error = null;
      try {
        const res = await invoke<InstallResult>("install_catalog_mod", {
          entry,
        });
        if (res.kind === "wad") {
          await this.loadModFromDir(res.mod_root);
        } else {
          const id = slugify(entry.name);
          if (!this.asiMods.some((m) => m.id === id)) {
            this.asiMods.push({
              id,
              name: entry.name,
              description: entry.description,
              version: res.version,
              modRoot: res.mod_root,
              asiFiles: res.asi_files,
            });
            this.enabled[id] = true;
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
        await this.refreshGame().catch(() => {});
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

    async launchGame() {
      if (!this.gameInfo) throw new Error("Set the game folder first");
      this.error = null;
      try {
        await invoke("launch_game", {
          exePath: this.gameInfo.exe_path,
          gameRoot: this.gameInfo.root,
        });
      } catch (e) {
        this.error = String(e);
        throw e;
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
