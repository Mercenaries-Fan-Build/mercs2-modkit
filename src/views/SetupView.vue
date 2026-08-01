<script setup lang="ts">
import { computed, ref } from "vue";
import { storeToRefs } from "pinia";
import { save } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "../stores/project";
import type { CrackResult, DxwrapperResult } from "../types";
import ProgressBar from "../components/ProgressBar.vue";

const store = useProjectStore();
const { gameInfo, busy, error, componentUpdates, pmcBbVersion, crackVersion, license } =
  storeToRefs(store);

const pmcBbUpdate = computed(() => componentUpdates.value["pmc_bb"]);
const crackUpdate = computed(() => componentUpdates.value["apply_crack"]);

// The UI shows the path detection resolved, but the user can switch between the
// crack and licensed (dxwrapper) flows. "drm_free" isn't an override — it's a
// fact about the exe (already DRM-free), so it's shown whenever it applies.
const pathOverride = ref<"licensed" | "crack" | null>(null);
const effectivePath = computed(() => {
  if (store.setupPath === "drm_free") return "drm_free";
  return pathOverride.value ?? store.setupPath;
});

const outputPath = ref<string | null>(null);
const showAnyway = ref(false);
const stage = ref("");
const pmcMsg = ref<string | null>(null);
const crackResult = ref<CrackResult | null>(null);
const dxResult = ref<DxwrapperResult | null>(null);
const updateResult = ref<CrackResult | null>(null);

async function installPmcBb() {
  stage.value = "Downloading pmc_bb.dll…";
  pmcMsg.value = null;
  try {
    const res = await store.installPmcBb();
    pmcMsg.value = `Installed pmc_bb.dll ${res.version} → ${res.path}`;
  } catch {
    /* surfaced via store.error */
  } finally {
    stage.value = "";
  }
}

async function pickOutput() {
  const f = await save({
    title: "Save cracked exe as…",
    defaultPath: "Mercenaries2.cracked.exe",
    filters: [{ name: "Executable", extensions: ["exe"] }],
  });
  if (typeof f === "string") outputPath.value = f;
}

async function runCrack() {
  stage.value = "Downloading apply_crack & patching…";
  crackResult.value = null;
  try {
    crackResult.value = await store.crackGame({ outputPath: outputPath.value });
  } catch {
    /* surfaced via store.error */
  } finally {
    stage.value = "";
  }
}

async function runUpdate() {
  stage.value = "Updating exe to v1.1 (official patch, no crack)…";
  updateResult.value = null;
  try {
    updateResult.value = await store.updateGame();
  } catch {
    /* surfaced via store.error */
  } finally {
    stage.value = "";
  }
}

async function runDxwrapper() {
  stage.value = "Downloading dxwrapper + logging pmc_bb…";
  dxResult.value = null;
  try {
    dxResult.value = await store.setupDxwrapper();
  } catch {
    /* surfaced via store.error */
  } finally {
    stage.value = "";
  }
}
</script>

<template>
  <div class="mx-auto max-w-3xl px-8 py-6">
    <header>
      <h2 class="plate-title text-xl">Game Setup</h2>
      <p class="text-sm text-zinc-500">
        Prepare the install for modding — no compiler or Python required.
      </p>
    </header>

    <div v-if="!gameInfo" class="empty-plate mt-10">
      Set your game folder in the bar above to begin.
    </div>

    <template v-else>
      <p class="mt-4 text-sm text-zinc-400">
        Detected
        <span class="text-zinc-200">{{ gameInfo.version }}</span>
        <span v-if="gameInfo.variant !== 'unknown'"> ({{ gameInfo.variant }})</span>.
      </p>

      <!-- ============================ PATH BANNER ============================ -->
      <div
        v-if="effectivePath === 'drm_free'"
        class="mt-3 rounded-xl border border-emerald-500/30 bg-emerald-500/10 p-4"
      >
        <p class="text-sm font-medium text-emerald-300">DRM-free copy — no crack needed ✓</p>
        <p class="mt-1 text-sm text-emerald-300/80">
          Your exe is already DRM-free and loads <span class="font-mono text-xs">pmc_bb.dll</span>
          itself, so there's nothing to crack or wrap — just install the loader below.
        </p>
      </div>
      <div
        v-else-if="effectivePath === 'licensed'"
        class="mt-3 rounded-xl border border-sky-500/30 bg-sky-500/10 p-4"
      >
        <p class="text-sm font-medium text-sky-300">Licensed copy — no exe changes needed ✓</p>
        <p class="mt-1 text-sm text-sky-300/80">
          {{
            license?.licensed
              ? "A SecuROM activation was found on this machine, so your exe already passes DRM."
              : "Using the non-destructive path: dxwrapper loads mods next to an untouched exe."
          }}
          Mods load via dxwrapper + a logging-only pmc_bb.dll — your
          <span class="font-mono text-xs">Mercenaries2.exe</span> is never cracked.
        </p>
        <button
          class="mt-2 text-xs text-zinc-400 underline hover:text-zinc-200"
          @click="pathOverride = 'crack'"
        >
          This isn't a licensed copy — crack the exe instead
        </button>
      </div>
      <div v-else class="mt-3 rounded-xl border border-amber-500/25 bg-amber-500/10 p-4">
        <p class="text-sm font-medium text-amber-200">Crack path</p>
        <p class="mt-1 text-sm text-amber-200/80">
          No SecuROM activation was detected, so this copy is set up by applying the
          SecuROM bypass to the exe.
        </p>
        <button
          class="mt-2 text-xs text-zinc-400 underline hover:text-zinc-200"
          @click="pathOverride = 'licensed'"
        >
          I own this copy (or it's DRM-free) — use dxwrapper instead, leave the exe alone
        </button>
      </div>

      <ProgressBar v-if="busy" indeterminate :label="stage" class="mt-4" />
      <div
        v-if="error"
        class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
      >
        {{ error }}
      </div>

      <!-- ============================ READY PANEL ============================ -->
      <div
        v-if="store.gameFullySetUp"
        class="mt-6 rounded-xl border border-emerald-500/30 bg-emerald-500/10 p-4"
      >
        <p class="text-sm font-medium text-emerald-300">Your game is ready ✓</p>
        <p class="mt-1 text-sm text-emerald-300/80">
          <template v-if="effectivePath === 'drm_free'">
            DRM-free exe + pmc_bb.dll — mods will load. No crack, no wrapper.
          </template>
          <template v-else-if="effectivePath === 'licensed'">
            dxwrapper + logging pmc_bb.dll installed — mods load without touching the exe.
          </template>
          <template v-else>
            v1.1, cracked, and pmc_bb.dll installed — no setup needed.
          </template>
        </p>
        <button
          class="mt-2 text-xs text-zinc-400 underline hover:text-zinc-200"
          @click="showAnyway = !showAnyway"
        >
          {{ showAnyway ? "Hide setup steps" : "Run setup anyway" }}
        </button>
      </div>

      <template v-if="!store.gameFullySetUp || showAnyway">
        <!-- ========================= DRM-FREE PATH ========================= -->
        <template v-if="effectivePath === 'drm_free'">
          <section class="guilloche mt-6 rounded-xl border border-zinc-800 p-5">
            <div class="flex items-start justify-between gap-4">
              <div>
                <h3 class="plate-title text-sm">Install pmc_bb.dll (ASI loader)</h3>
                <p class="mt-1 text-sm text-zinc-400">
                  Your DRM-free exe imports <span class="font-mono text-xs">pmc_bb.dll</span>
                  directly — install it and mods load. Nothing else needed.
                </p>
                <p class="mt-1 text-xs" :class="gameInfo.has_pmc_bb ? 'text-emerald-400' : 'text-zinc-500'">
                  {{ gameInfo.has_pmc_bb ? "Currently installed ✓" : "Not installed"
                  }}<span v-if="gameInfo.has_pmc_bb && pmcBbVersion"> ({{ pmcBbVersion }})</span>
                </p>
                <p
                  v-if="pmcBbUpdate?.available"
                  class="mt-1 flex items-center gap-1.5 text-xs font-medium text-amber-300"
                >
                  <span class="h-1.5 w-1.5 rounded-full bg-amber-400" /> New release → {{ pmcBbUpdate.latest }}
                </p>
              </div>
              <button
                class="shrink-0 rounded-lg px-3 py-2 text-sm font-medium text-white disabled:opacity-50"
                :class="pmcBbUpdate?.available ? 'bg-amber-500 text-zinc-900 hover:bg-amber-400' : 'bg-emerald-600 hover:bg-emerald-500'"
                :disabled="busy"
                @click="installPmcBb"
              >
                {{ pmcBbUpdate?.available ? "Update" : gameInfo.has_pmc_bb ? "Reinstall" : "Install" }}
              </button>
            </div>
            <p
              v-if="pmcMsg"
              class="mt-3 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-300"
            >
              {{ pmcMsg }}
            </p>
          </section>
        </template>

        <!-- ========================= LICENSED PATH ========================= -->
        <template v-else-if="effectivePath === 'licensed'">
          <!-- Optional: update v1.0 → v1.1 (official patch, keeps DRM) -->
          <section
            v-if="gameInfo.version === 'v1.0'"
            class="guilloche mt-6 rounded-xl border border-zinc-800 p-5"
          >
            <div class="flex items-start justify-between gap-4">
              <div>
                <h3 class="plate-title text-sm">Optional · Update to v1.1 (official patch)</h3>
                <p class="mt-1 text-sm text-zinc-400">
                  Applies EA's official v1.0 → v1.1 update — <span class="text-zinc-300">not a crack</span>.
                  The exe stays SecuROM-protected and your activation carries over. Your original is
                  backed up to <span class="font-mono text-xs">BACKUP/</span>.
                </p>
              </div>
              <button
                class="shrink-0 rounded-lg bg-zinc-700 px-3 py-2 text-sm font-medium text-white hover:bg-zinc-600 disabled:opacity-50"
                :disabled="busy"
                @click="runUpdate"
              >
                Update
              </button>
            </div>
            <p
              v-if="updateResult"
              class="mt-3 text-sm"
              :class="updateResult.ok ? 'text-emerald-400' : 'text-red-400'"
            >
              {{ updateResult.ok ? "Updated to v1.1 (DRM intact)" : "Update failed" }} —
              {{ updateResult.stdout || updateResult.stderr }}
            </p>
          </section>

          <section class="guilloche mt-4 rounded-xl border border-zinc-800 p-5">
            <div class="flex items-start justify-between gap-4">
              <div>
                <h3 class="plate-title text-sm">Set up dxwrapper (no exe changes)</h3>
                <p class="mt-1 text-sm text-zinc-400">
                  Downloads dxwrapper and a logging-only <span class="font-mono text-xs">pmc_bb.dll</span>
                  (no SecuROM spoof), and writes <span class="font-mono text-xs">dxwrapper.ini</span>.
                  dxwrapper wraps <span class="font-mono text-xs">d3d9.dll</span> — which the game
                  imports — and side-loads pmc_bb.dll, which loads your
                  <span class="font-mono text-xs">scripts</span> mods. Your exe stays byte-for-byte untouched.
                </p>
                <p class="mt-1 text-xs" :class="store.dxwrapperReady ? 'text-emerald-400' : 'text-zinc-500'">
                  {{
                    store.dxwrapperReady
                      ? "dxwrapper + pmc_bb.dll installed ✓"
                      : gameInfo.has_dxwrapper
                        ? "dxwrapper present; logging pmc_bb.dll missing"
                        : gameInfo.has_pmc_bb
                          ? "pmc_bb.dll present; dxwrapper missing"
                          : "Not installed"
                  }}<span v-if="store.dxwrapperReady && pmcBbVersion"> ({{ pmcBbVersion }})</span>
                </p>
              </div>
              <button
                class="shrink-0 rounded-lg bg-sky-600 px-3 py-2 text-sm font-medium text-white hover:bg-sky-500 disabled:opacity-50"
                :disabled="busy"
                @click="runDxwrapper"
              >
                {{ store.dxwrapperReady ? "Reinstall" : "Set up" }}
              </button>
            </div>

            <div v-if="dxResult" class="mt-4">
              <p class="text-sm" :class="dxResult.ok ? 'text-emerald-400' : 'text-red-400'">
                {{ dxResult.ok ? "Success" : "Failed" }} — dxwrapper {{ dxResult.version }}
              </p>
              <p class="mt-1 font-mono text-xs text-zinc-500">
                {{ dxResult.proxyPath }}<br />{{ dxResult.iniPath }}
              </p>
              <ul v-if="dxResult.notes.length" class="mt-2 list-disc pl-5 text-xs text-zinc-500">
                <li v-for="n in dxResult.notes" :key="n">{{ n }}</li>
              </ul>
            </div>
          </section>

          <p class="mt-4 text-xs text-zinc-600">
            The game launches your original <span class="font-mono">Mercenaries2.exe</span> —
            SecuROM stays intact and is satisfied by your activation. Uninstall by deleting
            <span class="font-mono">d3d9.dll</span>, <span class="font-mono">dxwrapper.dll</span>,
            and <span class="font-mono">dxwrapper.ini</span>.
          </p>
        </template>

        <!-- ============================ CRACK PATH ============================ -->
        <template v-else>
          <section class="guilloche mt-6 rounded-xl border border-zinc-800 p-5">
            <div class="flex items-start justify-between gap-4">
              <div>
                <h3 class="plate-title text-sm">1 · Install pmc_bb.dll (ASI loader)</h3>
                <p class="mt-1 text-sm text-zinc-400">
                  Our ASI loader + SecuROM spoof. Downloads the latest build and
                  places it next to the exe.
                </p>
                <p class="mt-1 text-xs" :class="gameInfo.has_pmc_bb ? 'text-emerald-400' : 'text-zinc-500'">
                  {{ gameInfo.has_pmc_bb ? "Currently installed ✓" : "Not installed"
                  }}<span v-if="gameInfo.has_pmc_bb && pmcBbVersion"> ({{ pmcBbVersion }})</span>
                </p>
                <p
                  v-if="pmcBbUpdate?.available"
                  class="mt-1 flex items-center gap-1.5 text-xs font-medium text-amber-300"
                >
                  <span class="h-1.5 w-1.5 rounded-full bg-amber-400" /> New release → {{ pmcBbUpdate.latest }}
                </p>
              </div>
              <button
                class="shrink-0 rounded-lg px-3 py-2 text-sm font-medium text-white disabled:opacity-50"
                :class="pmcBbUpdate?.available ? 'bg-amber-500 text-zinc-900 hover:bg-amber-400' : 'bg-emerald-600 hover:bg-emerald-500'"
                :disabled="busy"
                @click="installPmcBb"
              >
                {{ pmcBbUpdate?.available ? "Update" : gameInfo.has_pmc_bb ? "Reinstall" : "Install" }}
              </button>
            </div>
            <p
              v-if="pmcMsg"
              class="mt-3 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-300"
            >
              {{ pmcMsg }}
            </p>
          </section>

          <section class="guilloche mt-4 rounded-xl border border-zinc-800 p-5">
            <h3 class="plate-title text-sm">2 · Crack the exe</h3>
            <p class="mt-1 text-sm text-zinc-400">
              Applies the SecuROM bypass (auto-updating v1.0 → v1.1 first), writing a
              new cracked exe that loads pmc_bb.dll.
            </p>
            <p
              v-if="crackVersion"
              class="mt-1 text-xs"
              :class="crackUpdate?.available ? 'text-amber-300' : 'text-zinc-500'"
            >
              <template v-if="crackUpdate?.available">
                <span class="mr-1 inline-block h-1.5 w-1.5 rounded-full bg-amber-400 align-middle" />New
                apply_crack release → {{ crackUpdate.latest }} (you last ran {{ crackVersion }}).
              </template>
              <template v-else>Last ran apply_crack {{ crackVersion }}.</template>
            </p>

            <div class="mt-3">
              <label class="mb-1 block text-xs text-zinc-500">Output exe (optional)</label>
              <div class="flex gap-2">
                <input
                  :value="outputPath ?? ''"
                  readonly
                  placeholder="Default: Mercenaries2.cracked.exe next to the original"
                  class="field flex-1"
                />
                <button class="btn-outline" @click="pickOutput">Browse</button>
              </div>
            </div>

            <button class="btn-plate mt-4" :disabled="busy" @click="runCrack">Crack</button>

            <div v-if="crackResult" class="mt-4">
              <p class="text-sm" :class="crackResult.ok ? 'text-emerald-400' : 'text-red-400'">
                {{ crackResult.ok ? "Success" : "Failed" }} → {{ crackResult.output_path }}
              </p>
              <pre
                class="mt-2 max-h-60 overflow-auto rounded-lg border border-zinc-800 bg-black/40 p-3 text-xs text-zinc-400"
              >{{ crackResult.stdout || crackResult.stderr || "(no output)" }}</pre>
            </div>
          </section>

          <p class="mt-4 text-xs text-zinc-600">
            Tip: install pmc_bb.dll first, then crack — the cracked exe references
            pmc_bb.dll, which must be present in the folder.
          </p>
        </template>
      </template>
    </template>
  </div>
</template>
