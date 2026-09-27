<script setup lang="ts">
import { computed, ref } from "vue";
import { storeToRefs } from "pinia";
import { save } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "../stores/project";
import type { CrackResult } from "../types";

// The crack itself: run apply_crack on the base exe and write a cracked exe.
// Gated only on being done already (a v1.1 cracked build exists) — never on the
// exe's version or signing: any exe can be fed to the tool (it auto-updates
// v1.0 → v1.1 first), and whatever apply_crack makes of it is reported verbatim
// below, success or failure.
withDefaults(defineProps<{ title?: string }>(), {
  title: "Crack the exe (SecuROM bypass)",
});

const store = useProjectStore();
const { gameInfo, busy, crackVersion, componentUpdates } = storeToRefs(store);

const crackUpdate = computed(() => componentUpdates.value["apply_crack"]);

const outputPath = ref<string | null>(null);
const stage = ref("");
const result = ref<CrackResult | null>(null);

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
  result.value = null;
  try {
    result.value = await store.crackGame({ outputPath: outputPath.value });
  } catch {
    /* surfaced via store.error */
  } finally {
    stage.value = "";
  }
}
</script>

<template>
  <section v-if="gameInfo" class="guilloche rounded-xl border border-zinc-800 p-5">
    <h3 class="plate-title text-sm">{{ title }}</h3>
    <p class="mt-1 text-sm text-zinc-400">
      Applies the SecuROM bypass (auto-updating v1.0 → v1.1 first), writing a
      new cracked exe that loads pmc_bb.dll. The original exe is left as-is.
    </p>

    <dl class="mt-3 space-y-2 text-sm">
      <div class="flex items-center gap-3">
        <dt class="w-32 shrink-0 text-zinc-500">Input exe</dt>
        <dd class="min-w-0 break-all font-mono text-xs text-zinc-300">
          {{ gameInfo.exe_path }}
          <span class="text-zinc-500">— {{ gameInfo.version }} ({{ gameInfo.variant }})</span>
        </dd>
      </div>
      <div class="flex items-center gap-3">
        <dt class="w-32 shrink-0 text-zinc-500">Cracked build</dt>
        <dd class="min-w-0 break-all font-mono text-xs text-zinc-300">
          {{ store.crackedBuild?.path ?? "None found" }}
        </dd>
      </div>
      <div class="flex items-center gap-3">
        <dt class="w-32 shrink-0 text-zinc-500">apply_crack</dt>
        <dd class="text-zinc-300">
          {{ crackVersion ?? "not downloaded yet" }}
          <span class="text-zinc-500">· latest {{ crackUpdate?.latest || "—" }}</span>
        </dd>
      </div>
    </dl>

    <p
      v-if="crackUpdate?.available"
      class="mt-3 flex items-center gap-1.5 text-sm font-medium text-amber-300"
    >
      <span class="h-1.5 w-1.5 rounded-full bg-amber-400" />
      New apply_crack release → {{ crackUpdate.latest }} — the next crack uses it.
    </p>

    <p v-if="gameInfo.has_dxwrapper" class="mt-3 text-xs text-amber-300/90">
      dxwrapper is installed, so modkit launches the stock exe and ignores any
      cracked exe. Remove dxwrapper for the cracked exe to be the one launched.
    </p>

    <p v-if="store.crackedBuild" class="mt-3 text-sm text-emerald-300/80">
      Already cracked ✓ — nothing more to do here.
    </p>
    <template v-else>
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
    </template>
    <p v-if="stage" class="mt-2 text-xs text-zinc-500">{{ stage }}</p>

    <div v-if="result" class="mt-4">
      <p class="text-sm" :class="result.ok ? 'text-emerald-400' : 'text-red-400'">
        {{ result.ok ? "Success" : "Failed" }} → {{ result.outputPath }}
        <span class="text-zinc-500">(apply_crack {{ result.toolVersion }})</span>
      </p>
      <pre
        class="mt-2 max-h-60 overflow-auto rounded-lg border border-zinc-800 bg-black/40 p-3 text-xs text-zinc-400"
      >{{ result.stdout || result.stderr || "(no output)" }}</pre>
    </div>

    <p v-if="!store.crackedBuild" class="mt-4 text-xs text-zinc-600">
      Install pmc_bb.dll first, then crack — the cracked exe imports pmc_bb.dll,
      which must be present in the folder.
    </p>
  </section>
</template>
