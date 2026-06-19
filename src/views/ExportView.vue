<script setup lang="ts">
import { ref, watchEffect } from "vue";
import { storeToRefs } from "pinia";
import { open } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "../stores/project";
import ProgressBar from "../components/ProgressBar.vue";

const store = useProjectStore();
const { busy, error, buildResult, validation, unresolvedCount, gameInfo } =
  storeToRefs(store);

const outputDir = ref("");

// Default the output folder to the detected game's data dir, once known.
watchEffect(() => {
  if (!outputDir.value && gameInfo.value?.data_dir) {
    outputDir.value = gameInfo.value.data_dir;
  }
});
const splitByPatch = ref(false);
const mergeInto = ref<string | null>(null);
const simulatorPath = ref<string | null>(null);
const stage = ref<string>("");

async function pickOutputDir() {
  const dir = await open({ directory: true, title: "Select output folder" });
  if (typeof dir === "string") outputDir.value = dir;
}

async function pickMergeWad() {
  const f = await open({
    title: "Select existing vz-patch.wad to merge into",
    filters: [{ name: "WAD", extensions: ["wad"] }],
  });
  if (typeof f === "string") mergeInto.value = f;
}

async function buildAndValidate() {
  if (!outputDir.value) {
    store.error = "Choose an output folder first.";
    return;
  }
  store.error = null;
  stage.value = "Assembling patch WAD…";
  const result = await store
    .assemble({
      outputDir: outputDir.value,
      splitByPatch: splitByPatch.value,
      mergeInto: mergeInto.value,
    })
    .catch(() => null);
  if (!result || result.outputs.length === 0) {
    stage.value = "";
    return;
  }

  // Validate the first produced WAD with wad_simulator.
  stage.value = "Validating with wad_simulator…";
  try {
    let sim = simulatorPath.value;
    if (!sim) {
      // Try to fetch the release binary; fall back to PATH if it fails.
      sim = await store.fetchSimulator().catch(() => null);
      if (sim) simulatorPath.value = sim;
    }
    await store.validate(result.outputs[0].path, sim);
  } catch {
    /* surfaced via store.error */
  } finally {
    stage.value = "";
  }
}

function fmtBytes(n: number): string {
  if (n > 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MB`;
  if (n > 1 << 10) return `${(n / (1 << 10)).toFixed(1)} KB`;
  return `${n} B`;
}
</script>

<template>
  <div class="mx-auto max-w-3xl px-8 py-6">
    <header>
      <h2 class="text-xl font-semibold">Build &amp; Deploy</h2>
      <p class="text-sm text-zinc-500">
        Assemble loaded mods into a patch WAD, then validate before deploying.
      </p>
    </header>

    <div
      v-if="store.enabledMods.length === 0"
      class="mt-10 rounded-xl border border-dashed border-zinc-800 px-8 py-16 text-center text-zinc-500"
    >
      Enable at least one mod to build.
    </div>

    <template v-else>
      <div
        v-if="unresolvedCount > 0"
        class="mt-4 rounded-lg border border-amber-500/30 bg-amber-500/10 px-4 py-3 text-sm text-amber-300"
      >
        {{ unresolvedCount }} unresolved conflict{{
          unresolvedCount === 1 ? "" : "s"
        }}
        — unresolved assets default to load order (first mod wins).
      </div>

      <!-- Settings -->
      <section class="mt-6 space-y-4 rounded-xl border border-zinc-800 p-5">
        <div>
          <label class="mb-1 block text-sm text-zinc-400">Output folder</label>
          <div class="flex gap-2">
            <input
              v-model="outputDir"
              readonly
              placeholder="Choose a folder…"
              class="flex-1 rounded-lg border border-zinc-700 bg-zinc-900 px-3 py-2 text-sm"
            />
            <button class="btn-secondary" @click="pickOutputDir">Browse</button>
          </div>
        </div>

        <label class="flex items-center gap-2 text-sm text-zinc-300">
          <input v-model="splitByPatch" type="checkbox" class="accent-emerald-500" />
          Split into one WAD per patch group
        </label>

        <div>
          <label class="mb-1 block text-sm text-zinc-400">
            Merge into existing WAD (optional)
          </label>
          <div class="flex gap-2">
            <input
              :value="mergeInto ?? ''"
              readonly
              placeholder="None — build fresh"
              class="flex-1 rounded-lg border border-zinc-700 bg-zinc-900 px-3 py-2 text-sm"
            />
            <button class="btn-secondary" @click="pickMergeWad">Browse</button>
            <button
              v-if="mergeInto"
              class="btn-secondary"
              @click="mergeInto = null"
            >
              Clear
            </button>
          </div>
        </div>
      </section>

      <button
        class="mt-5 w-full rounded-lg bg-emerald-600 px-4 py-2.5 font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
        :disabled="busy"
        @click="buildAndValidate"
      >
        Build &amp; Validate
      </button>

      <ProgressBar v-if="busy" indeterminate :label="stage" class="mt-4" />

      <div
        v-if="error"
        class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
      >
        {{ error }}
      </div>

      <!-- Build result -->
      <section v-if="buildResult" class="mt-6">
        <h3 class="mb-2 text-sm font-medium text-zinc-300">Output</h3>
        <div class="space-y-2">
          <div
            v-for="o in buildResult.outputs"
            :key="o.path"
            class="rounded-lg border border-zinc-800 bg-zinc-900/50 px-4 py-3 text-sm"
          >
            <p class="font-mono text-zinc-200">{{ o.path }}</p>
            <p class="text-xs text-zinc-500">
              group “{{ o.patch_group }}” · {{ o.block_count }} block{{
                o.block_count === 1 ? "" : "s"
              }}
              · {{ fmtBytes(o.byte_size) }}
            </p>
          </div>
        </div>
      </section>

      <!-- Validation result -->
      <section v-if="validation" class="mt-6">
        <h3 class="mb-2 flex items-center gap-2 text-sm font-medium text-zinc-300">
          Validation
          <span
            class="rounded-full px-2 py-0.5 text-xs"
            :class="
              validation.ok
                ? 'bg-emerald-500/15 text-emerald-300'
                : 'bg-red-500/15 text-red-300'
            "
          >
            {{ validation.ok ? "passed" : "failed" }}
            (exit {{ validation.exit_code ?? "?" }})
          </span>
        </h3>
        <pre
          class="max-h-72 overflow-auto rounded-lg border border-zinc-800 bg-black/40 p-3 text-xs text-zinc-400"
        >{{ validation.stdout || validation.stderr || "(no output)" }}</pre>
      </section>
    </template>
  </div>
</template>

<style scoped>
.btn-secondary {
  border-radius: 0.5rem;
  border: 1px solid rgb(63 63 70);
  padding: 0.5rem 0.75rem;
  font-size: 0.875rem;
  color: rgb(212 212 216);
}
.btn-secondary:hover {
  background-color: rgb(39 39 42);
}
</style>
