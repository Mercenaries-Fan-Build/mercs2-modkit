<script setup lang="ts">
import { onMounted, ref } from "vue";
import { storeToRefs } from "pinia";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useProjectStore } from "../stores/project";
import type { CatalogEntry } from "../types";
import ProgressBar from "../components/ProgressBar.vue";

const store = useProjectStore();
const { catalog, catalogSource, busy, error } = storeToRefs(store);

const installing = ref<string | null>(null);
const lastInstalled = ref<string | null>(null);

onMounted(() => {
  if (store.catalog.length === 0) store.fetchCatalog();
});

async function install(entry: CatalogEntry) {
  installing.value = entry.name;
  lastInstalled.value = null;
  try {
    const res = await store.installFromCatalog(entry);
    lastInstalled.value = `Installed ${entry.name} ${res.version} (${res.kind.toUpperCase()}, ${res.staged_files} file${res.staged_files === 1 ? "" : "s"})`;
  } catch {
    /* surfaced via store.error */
  } finally {
    installing.value = null;
  }
}
</script>

<template>
  <div class="mx-auto max-w-3xl px-8 py-6">
    <header class="flex items-center justify-between">
      <div>
        <h2 class="text-xl font-semibold">Browse Catalog</h2>
        <p class="text-sm text-zinc-500">
          Curated mods. Installing pulls the repo's latest release artifacts into
          staging.
        </p>
      </div>
      <button
        class="rounded-md px-2 py-1 text-xs text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200 disabled:opacity-50"
        :disabled="busy"
        @click="store.fetchCatalog()"
      >
        Refresh
      </button>
    </header>

    <p v-if="catalogSource" class="mt-1 text-xs text-zinc-600">
      source: {{ catalogSource }}
    </p>

    <ProgressBar
      v-if="busy && !installing"
      indeterminate
      label="Loading catalog…"
      class="mt-4"
    />
    <ProgressBar
      v-if="installing"
      indeterminate
      :label="`Installing ${installing}…`"
      class="mt-4"
    />

    <div
      v-if="lastInstalled"
      class="mt-4 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-4 py-3 text-sm text-emerald-300"
    >
      {{ lastInstalled }}
    </div>
    <div
      v-if="error"
      class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
    >
      {{ error }}
    </div>

    <ul v-if="catalog.length" class="mt-6 space-y-3">
      <li
        v-for="entry in catalog"
        :key="entry.repository"
        class="rounded-xl border border-zinc-800 bg-zinc-900/50 p-4"
      >
        <div class="flex items-start justify-between gap-4">
          <div class="min-w-0">
            <p class="font-medium text-zinc-100">{{ entry.name }}</p>
            <p class="mt-0.5 text-sm text-zinc-400">{{ entry.description }}</p>
            <button
              class="mt-1 truncate text-xs text-sky-400 hover:underline"
              @click="openUrl(entry.repository)"
            >
              {{ entry.repository }}
            </button>
          </div>
          <button
            class="shrink-0 rounded-lg bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
            :disabled="busy"
            @click="install(entry)"
          >
            Install
          </button>
        </div>
      </li>
    </ul>

    <div
      v-else-if="!busy"
      class="mt-10 rounded-xl border border-dashed border-zinc-800 px-8 py-16 text-center"
    >
      <p class="text-zinc-400">The catalog is empty.</p>
      <p class="mt-1 text-sm text-zinc-600">
        Add entries to <code class="text-zinc-400">registry.json</code> — each
        with a name, description, and git repository.
      </p>
    </div>
  </div>
</template>
