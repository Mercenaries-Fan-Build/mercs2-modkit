<script setup lang="ts">
import { storeToRefs } from "pinia";
import { RouterLink } from "vue-router";
import { open } from "@tauri-apps/plugin-dialog";
import { Switch } from "@headlessui/vue";
import { useProjectStore } from "../stores/project";
import type { AsiMod } from "../types";
import ConflictBadge from "../components/ConflictBadge.vue";
import ProgressBar from "../components/ProgressBar.vue";

const store = useProjectStore();
const { mods, asiMods, busy, error, activeAssetCount, conflictCount, gameInfo } =
  storeToRefs(store);

const ASI_TARGETS = [
  { value: "scripts", label: "scripts/" },
  { value: ".", label: "game root" },
  { value: "plugins", label: "plugins/" },
  { value: "update", label: "update/" },
];

async function addMod() {
  const dir = await open({ directory: true, title: "Select a mod folder" });
  if (typeof dir === "string") {
    await store.loadModFromDir(dir).catch(() => {});
  }
}

async function addPlugin() {
  const sel = await open({
    multiple: true,
    title: "Select .asi plugin(s)",
    filters: [{ name: "ASI plugin", extensions: ["asi"] }],
  });
  const paths = Array.isArray(sel) ? sel : typeof sel === "string" ? [sel] : [];
  if (paths.length) await store.importLocalAsi(paths).catch(() => {});
}

async function deploy(mod: AsiMod) {
  await store.deployAsiMod(mod).catch(() => {});
}

async function deployEnabled() {
  for (const m of asiMods.value) {
    if (store.isEnabled(m.id)) await store.deployAsiMod(m).catch(() => {});
  }
}
</script>

<template>
  <div class="mx-auto max-w-4xl px-8 py-6">
    <header class="flex items-center justify-between">
      <div>
        <h2 class="text-xl font-semibold">Mod Library</h2>
        <p class="text-sm text-zinc-500">
          {{ asiMods.length }} ASI · {{ mods.length }} WAD ·
          {{ activeAssetCount }} asset{{ activeAssetCount === 1 ? "" : "s" }}
        </p>
      </div>
      <div class="flex items-center gap-3">
        <ConflictBadge v-if="mods.length" :count="conflictCount" />
        <RouterLink
          to="/catalog"
          class="rounded-lg border border-zinc-700 px-3 py-2 text-sm text-zinc-300 hover:bg-zinc-800"
        >
          Browse Catalog
        </RouterLink>
        <button
          class="rounded-lg border border-zinc-700 px-3 py-2 text-sm text-zinc-300 hover:bg-zinc-800 disabled:opacity-50"
          :disabled="busy"
          @click="addPlugin"
        >
          Add Plugin
        </button>
        <button
          class="rounded-lg bg-emerald-600 px-4 py-2 text-sm font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
          :disabled="busy"
          @click="addMod"
        >
          Add Folder
        </button>
      </div>
    </header>

    <ProgressBar v-if="busy" indeterminate class="mt-4" />

    <div
      v-if="error"
      class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
    >
      {{ error }}
    </div>

    <!-- ASI plugins -->
    <section v-if="asiMods.length" class="mt-6">
      <div class="mb-2 flex items-center justify-between">
        <h3 class="text-sm font-medium text-zinc-300">ASI Plugins</h3>
        <div class="flex items-center gap-2 text-xs">
          <label class="text-zinc-500">Deploy to</label>
          <select
            :value="store.asiTarget"
            class="rounded border border-zinc-700 bg-zinc-900 px-2 py-1 text-zinc-200"
            @change="store.setAsiTarget(($event.target as HTMLSelectElement).value)"
          >
            <option v-for="t in ASI_TARGETS" :key="t.value" :value="t.value">
              {{ t.label }}
            </option>
          </select>
          <button
            class="rounded-md bg-emerald-600 px-2 py-1 font-medium text-white hover:bg-emerald-500 disabled:opacity-40"
            :disabled="busy || !gameInfo"
            :title="!gameInfo ? 'Set the game folder first' : ''"
            @click="deployEnabled"
          >
            Deploy enabled
          </button>
        </div>
      </div>

      <ul class="space-y-2">
        <li
          v-for="m in asiMods"
          :key="m.id"
          class="flex items-center gap-3 rounded-xl border border-zinc-800 bg-zinc-900/50 px-4 py-3"
          :class="{ 'opacity-50': !store.isEnabled(m.id) }"
        >
          <Switch
            :model-value="store.isEnabled(m.id)"
            @update:model-value="store.toggleMod(m.id)"
            :class="store.isEnabled(m.id) ? 'bg-emerald-600' : 'bg-zinc-700'"
            class="relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition"
          >
            <span class="sr-only">Enable {{ m.name }}</span>
            <span
              class="inline-block h-3.5 w-3.5 transform rounded-full bg-white transition"
              :class="store.isEnabled(m.id) ? 'translate-x-[18px]' : 'translate-x-0.5'"
            />
          </Switch>

          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span class="font-medium text-zinc-100">{{ m.name }}</span>
              <span
                v-if="store.isAsiDeployed(m)"
                class="rounded-full bg-emerald-500/15 px-2 py-0.5 text-[11px] text-emerald-300"
              >
                deployed
              </span>
              <span
                v-else
                class="rounded-full bg-zinc-700/40 px-2 py-0.5 text-[11px] text-zinc-400"
              >
                staged
              </span>
            </div>
            <p class="truncate text-xs text-zinc-500">
              v{{ m.version }} ·
              {{ m.asiFiles.length }} plugin{{ m.asiFiles.length === 1 ? "" : "s" }}
              ({{ m.asiFiles.join(", ") }})
            </p>
          </div>

          <button
            class="rounded-md bg-emerald-600/90 px-2.5 py-1 text-xs font-medium text-white hover:bg-emerald-500 disabled:opacity-40"
            :disabled="busy || !gameInfo || !store.isEnabled(m.id)"
            :title="!gameInfo ? 'Set the game folder first' : ''"
            @click="deploy(m)"
          >
            Deploy
          </button>
          <button
            class="rounded-md px-2 py-1 text-xs text-zinc-500 hover:bg-red-500/10 hover:text-red-400"
            @click="store.removeAsiMod(m.id)"
          >
            Remove
          </button>
        </li>
      </ul>
    </section>

    <!-- WAD-asset mods -->
    <section v-if="mods.length" class="mt-8">
      <h3 class="text-sm font-medium text-zinc-300">WAD-Asset Mods</h3>
      <p v-if="mods.length > 1" class="mt-1 text-xs text-zinc-500">
        Load order — the <span class="text-zinc-300">top</span> mod wins
        conflicts. Reorder with the arrows.
      </p>

      <ul class="mt-2 space-y-2">
        <li
          v-for="(m, i) in mods"
          :key="m.id"
          class="flex items-center gap-3 rounded-xl border border-zinc-800 bg-zinc-900/50 px-4 py-3"
          :class="{ 'opacity-50': !store.isEnabled(m.id) }"
        >
          <div class="flex flex-col">
            <button
              class="text-zinc-600 hover:text-zinc-300 disabled:opacity-30"
              :disabled="i === 0"
              title="Move up"
              @click="store.moveMod(m.id, 'up')"
            >
              ▲
            </button>
            <button
              class="text-zinc-600 hover:text-zinc-300 disabled:opacity-30"
              :disabled="i === mods.length - 1"
              title="Move down"
              @click="store.moveMod(m.id, 'down')"
            >
              ▼
            </button>
          </div>

          <span class="w-5 text-center text-xs text-zinc-600">{{ i + 1 }}</span>

          <Switch
            :model-value="store.isEnabled(m.id)"
            @update:model-value="store.toggleMod(m.id)"
            :class="store.isEnabled(m.id) ? 'bg-emerald-600' : 'bg-zinc-700'"
            class="relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition"
          >
            <span class="sr-only">Enable {{ m.manifest.name }}</span>
            <span
              class="inline-block h-3.5 w-3.5 transform rounded-full bg-white transition"
              :class="store.isEnabled(m.id) ? 'translate-x-[18px]' : 'translate-x-0.5'"
            />
          </Switch>

          <div class="min-w-0 flex-1">
            <RouterLink
              :to="`/mod/${m.id}`"
              class="font-medium text-zinc-100 hover:text-emerald-400"
            >
              {{ m.manifest.name }}
            </RouterLink>
            <p class="truncate text-xs text-zinc-500">
              v{{ m.manifest.version }}
              <span v-if="m.manifest.author"> · {{ m.manifest.author }}</span>
              · {{ m.assets.length }} asset{{ m.assets.length === 1 ? "" : "s" }}
            </p>
          </div>

          <button
            class="rounded-md px-2 py-1 text-xs text-zinc-500 hover:bg-red-500/10 hover:text-red-400"
            @click="store.removeMod(m.id)"
          >
            Remove
          </button>
        </li>
      </ul>
    </section>

    <!-- Empty -->
    <div
      v-if="!mods.length && !asiMods.length"
      class="mt-10 rounded-xl border border-dashed border-zinc-800 px-8 py-16 text-center"
    >
      <p class="text-zinc-400">No mods yet.</p>
      <p class="mt-1 text-sm text-zinc-600">
        <RouterLink to="/catalog" class="text-emerald-400 hover:underline"
          >Browse the catalog</RouterLink
        >
        or add a folder containing a
        <code class="text-zinc-400">manifest.json</code> or
        <code class="text-zinc-400">.asi</code> plugin.
      </p>
    </div>
  </div>
</template>
