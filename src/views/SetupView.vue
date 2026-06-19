<script setup lang="ts">
import { ref } from "vue";
import { storeToRefs } from "pinia";
import { save } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "../stores/project";
import type { CrackResult } from "../types";
import ProgressBar from "../components/ProgressBar.vue";

const store = useProjectStore();
const { gameInfo, busy, error } = storeToRefs(store);

const updateToV11 = ref(true);
const outputPath = ref<string | null>(null);
const stage = ref("");
const pmcMsg = ref<string | null>(null);
const crackResult = ref<CrackResult | null>(null);

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
    crackResult.value = await store.crackGame({
      updateToV11: updateToV11.value,
      outputPath: outputPath.value,
    });
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
      <h2 class="text-xl font-semibold">Game Setup</h2>
      <p class="text-sm text-zinc-500">
        Prepare the install for modding — no compiler or Python required.
      </p>
    </header>

    <div
      v-if="!gameInfo"
      class="mt-10 rounded-xl border border-dashed border-zinc-800 px-8 py-16 text-center text-zinc-500"
    >
      Set your game folder in the bar above to begin.
    </div>

    <template v-else>
      <p class="mt-4 text-sm text-zinc-400">
        Detected
        <span class="text-zinc-200">{{ gameInfo.version }}</span>
        <span v-if="gameInfo.variant !== 'unknown'"> ({{ gameInfo.variant }})</span>.
      </p>

      <ProgressBar v-if="busy" indeterminate :label="stage" class="mt-4" />
      <div
        v-if="error"
        class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
      >
        {{ error }}
      </div>

      <!-- Step 1: pmc_bb.dll -->
      <section class="mt-6 rounded-xl border border-zinc-800 p-5">
        <div class="flex items-start justify-between gap-4">
          <div>
            <h3 class="font-medium text-zinc-100">
              1 · Install pmc_bb.dll (ASI loader)
            </h3>
            <p class="mt-1 text-sm text-zinc-400">
              Our ASI loader + SecuROM spoof. Downloads the latest build and
              places it next to the exe.
            </p>
            <p class="mt-1 text-xs" :class="gameInfo.has_pmc_bb ? 'text-emerald-400' : 'text-zinc-500'">
              {{ gameInfo.has_pmc_bb ? "Currently installed ✓" : "Not installed" }}
            </p>
          </div>
          <button
            class="shrink-0 rounded-lg bg-emerald-600 px-3 py-2 text-sm font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
            :disabled="busy"
            @click="installPmcBb"
          >
            {{ gameInfo.has_pmc_bb ? "Reinstall" : "Install" }}
          </button>
        </div>
        <p
          v-if="pmcMsg"
          class="mt-3 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-300"
        >
          {{ pmcMsg }}
        </p>
      </section>

      <!-- Step 2: crack / update -->
      <section class="mt-4 rounded-xl border border-zinc-800 p-5">
        <h3 class="font-medium text-zinc-100">2 · Crack / update the exe</h3>
        <p class="mt-1 text-sm text-zinc-400">
          Applies the SecuROM bypass (and optionally updates v1.0 → v1.1),
          writing a new cracked exe that loads pmc_bb.dll.
        </p>

        <label class="mt-3 flex items-center gap-2 text-sm text-zinc-300">
          <input v-model="updateToV11" type="checkbox" class="accent-emerald-500" />
          Update to v1.1 if needed
        </label>

        <div class="mt-3">
          <label class="mb-1 block text-xs text-zinc-500">Output exe (optional)</label>
          <div class="flex gap-2">
            <input
              :value="outputPath ?? ''"
              readonly
              placeholder="Default: Mercenaries2.cracked.exe next to the original"
              class="flex-1 rounded-lg border border-zinc-700 bg-zinc-900 px-3 py-2 text-sm"
            />
            <button
              class="rounded-lg border border-zinc-700 px-3 py-2 text-sm text-zinc-300 hover:bg-zinc-800"
              @click="pickOutput"
            >
              Browse
            </button>
          </div>
        </div>

        <button
          class="mt-4 rounded-lg bg-emerald-600 px-3 py-2 text-sm font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
          :disabled="busy"
          @click="runCrack"
        >
          Crack / update
        </button>

        <div v-if="crackResult" class="mt-4">
          <p
            class="text-sm"
            :class="crackResult.ok ? 'text-emerald-400' : 'text-red-400'"
          >
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
  </div>
</template>
