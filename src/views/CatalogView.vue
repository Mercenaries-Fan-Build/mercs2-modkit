<script setup lang="ts">
import { onMounted, ref } from "vue";
import { storeToRefs } from "pinia";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useProjectStore } from "../stores/project";
import type { CatalogMod } from "../types";
import ProgressBar from "../components/ProgressBar.vue";

const store = useProjectStore();
const { catalog, catalogSource, busy, error, gameInfo } = storeToRefs(store);

const working = ref<string | null>(null); // repository#slug currently acting on
const lastAction = ref<string | null>(null);

onMounted(() => {
  if (store.catalog.length === 0) store.fetchCatalog();
});

function keyOf(item: CatalogMod): string {
  return `${item.repository}#${item.slug}`;
}

async function download(item: CatalogMod) {
  working.value = keyOf(item);
  lastAction.value = null;
  try {
    const res = await store.downloadFromCatalog(item);
    lastAction.value = `Downloaded ${item.name} ${res.version} (${res.staged_files} file${res.staged_files === 1 ? "" : "s"}) — enable it to deploy`;
  } catch {
    /* surfaced via store.error */
  } finally {
    working.value = null;
  }
}

async function enable(item: CatalogMod) {
  const lib = store.catalogLibMod(item);
  if (lib) store.setModEnabled(lib.id, true);
}

async function deploy(item: CatalogMod) {
  const lib = store.catalogLibMod(item);
  if (!lib) return;
  working.value = keyOf(item);
  try {
    await store.deployAsiMod(lib);
    lastAction.value = `Deployed ${item.name}`;
  } catch {
    /* surfaced via store.error */
  } finally {
    working.value = null;
  }
}

// Pull the newer release into the Library, preserving enabled/deployed state.
async function update(item: CatalogMod) {
  const lib = store.catalogLibMod(item);
  if (!lib) return;
  working.value = keyOf(item);
  lastAction.value = null;
  try {
    await store.updateAsiMod(lib);
    lastAction.value = `Updated ${item.name} → v${item.version}`;
  } catch {
    /* surfaced via store.error */
  } finally {
    working.value = null;
  }
}
</script>

<template>
  <div class="mx-auto max-w-3xl px-8 py-6">
    <header>
      <h2 class="text-xl font-semibold">Browse Catalog</h2>
      <p class="text-sm text-zinc-500">
        Mods from curated repositories. Download → enable → deploy; state is
        reconciled against your game folder.
      </p>
    </header>

    <p v-if="catalogSource" class="mt-1 text-xs text-zinc-600">
      source: {{ catalogSource }}
    </p>

    <button
      class="mt-4 rounded-md border border-zinc-700 px-3 py-1.5 text-xs text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200 disabled:opacity-50"
      :disabled="busy"
      @click="store.fetchCatalog()"
    >
      Refresh
    </button>

    <ProgressBar
      v-if="busy && !working"
      indeterminate
      label="Loading catalog…"
      class="mt-4"
    />
    <ProgressBar
      v-if="working"
      indeterminate
      label="Working…"
      class="mt-4"
    />

    <div
      v-if="lastAction"
      class="mt-4 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-4 py-3 text-sm text-emerald-300"
    >
      {{ lastAction }}
    </div>
    <div
      v-if="error"
      class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
    >
      {{ error }}
    </div>

    <ul v-if="catalog.length" class="mt-6 space-y-3">
      <li
        v-for="item in catalog"
        :key="`${item.repository}#${item.slug}`"
        class="rounded-xl border border-zinc-800 bg-zinc-900/50 p-4"
      >
        <div class="flex items-start justify-between gap-4">
          <div class="min-w-0">
            <div class="flex items-center gap-2">
              <p class="font-medium text-zinc-100">{{ item.name }}</p>
              <span
                v-if="item.version"
                class="rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-400"
                >v{{ item.version }}</span
              >
              <span
                class="rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] uppercase text-zinc-500"
                >{{ item.kind }}</span
              >
              <!-- A newer release than the installed Library copy exists -->
              <span
                v-if="store.catalogUpdate(item)"
                class="rounded-full bg-amber-500/15 px-2 py-0.5 text-[10px] font-medium text-amber-300"
                :title="`A newer release (v${store.catalogUpdate(item)}) is available — your Library copy is older`"
                >update available</span
              >
              <!-- Lifecycle state, reconciled against the game folder -->
              <span
                v-if="store.catalogModState(item) === 'deployed'"
                class="rounded-full bg-violet-500/15 px-2 py-0.5 text-[10px] text-violet-300"
                >deployed</span
              >
              <span
                v-else-if="store.catalogModState(item) === 'enabled'"
                class="rounded-full bg-emerald-500/15 px-2 py-0.5 text-[10px] text-emerald-300"
                >enabled · not deployed</span
              >
              <span
                v-else-if="store.catalogModState(item) === 'downloaded'"
                class="rounded-full bg-zinc-700/50 px-2 py-0.5 text-[10px] text-zinc-300"
                >downloaded · disabled</span
              >
            </div>
            <p class="mt-0.5 text-sm text-zinc-400">{{ item.description }}</p>
            <button
              class="mt-1 truncate text-xs text-sky-400 hover:underline"
              @click="openUrl(item.repository)"
            >
              {{ item.repo_name }} · {{ item.repository }}
            </button>
          </div>

          <!-- Action depends on lifecycle state: download -> enable -> deploy -->
          <div class="flex shrink-0 items-center gap-2">
            <!-- A newer release exists for an already-downloaded mod -->
            <button
              v-if="store.catalogUpdate(item)"
              class="rounded-lg bg-amber-500 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-amber-400 disabled:opacity-50"
              :disabled="busy"
              :title="`Update the Library copy to v${store.catalogUpdate(item)} and redeploy if deployed`"
              @click="update(item)"
            >
              Update → v{{ store.catalogUpdate(item) }}
            </button>
            <button
              v-if="store.catalogModState(item) === 'none'"
              class="rounded-lg bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
              :disabled="busy"
              title="Download this mod's release asset(s) into the Library"
              @click="download(item)"
            >
              Download
            </button>
            <button
              v-else-if="store.catalogModState(item) === 'downloaded'"
              class="rounded-lg bg-sky-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-500 disabled:opacity-50"
              :disabled="busy"
              title="Mark this mod for deployment"
              @click="enable(item)"
            >
              Enable
            </button>
            <button
              v-else-if="store.catalogModState(item) === 'enabled'"
              class="rounded-lg bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
              :disabled="busy || !gameInfo"
              :title="!gameInfo ? 'Set the game folder first' : 'Copy into the game folder'"
              @click="deploy(item)"
            >
              Deploy
            </button>
            <span
              v-else
              class="text-xs font-medium text-violet-300"
              title="This mod's plugin is present in the game folder"
              >✓ in game</span
            >
          </div>
        </div>
      </li>
    </ul>

    <div
      v-else-if="!busy"
      class="mt-10 rounded-xl border border-dashed border-zinc-800 px-8 py-16 text-center"
    >
      <p class="text-zinc-400">The catalog is empty.</p>
      <p class="mt-1 text-sm text-zinc-600">
        Add repository sources to
        <code class="text-zinc-400">registry.json</code>. Each repo lists its mods
        in <code class="text-zinc-400">repository.json</code> (objects with name,
        description, type, and release assets).
      </p>
    </div>
  </div>
</template>
