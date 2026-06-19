<script setup lang="ts">
import { storeToRefs } from "pinia";
import { RouterLink } from "vue-router";
import { open, ask } from "@tauri-apps/plugin-dialog";
import { Switch } from "@headlessui/vue";
import { useProjectStore } from "../stores/project";
import type { AsiMod, DeployedAsi } from "../types";
import ConflictBadge from "../components/ConflictBadge.vue";
import ProgressBar from "../components/ProgressBar.vue";

const store = useProjectStore();
const { mods, asiMods, busy, error, activeAssetCount, conflictCount, gameInfo } =
  storeToRefs(store);

/** Deploy lifecycle status of a library ASI mod, for the status pill. */
type AsiStatus = { label: string; cls: string };
function asiStatus(m: AsiMod): AsiStatus {
  if (store.isAsiDeployed(m))
    return { label: "deployed", cls: "bg-emerald-500/15 text-emerald-300" };
  if (!store.isEnabled(m.id))
    return { label: "disabled", cls: "bg-zinc-700/40 text-zinc-400" };
  if (!gameInfo.value)
    return { label: "enabled · no game set", cls: "bg-amber-500/15 text-amber-300" };
  return { label: "ready to deploy", cls: "bg-sky-500/15 text-sky-300" };
}

async function undeploy(m: AsiMod) {
  const ok = await ask(
    `Remove ${m.name}'s plugin(s) from the game folder?\nThey'll be moved to modkit's trash (recoverable).`,
    { title: "Undeploy", kind: "warning" }
  );
  if (ok) await store.undeployAsiMod(m).catch(() => {});
}

async function removeFromLibrary(m: AsiMod) {
  if (store.isAsiDeployed(m)) {
    const ok = await ask(
      `${m.name} is still deployed in the game folder.\nRemove it from the game (to trash) and forget it from the Library?`,
      { title: "Remove mod", kind: "warning" }
    );
    if (ok) await store.forceRemoveAsiMod(m).catch(() => {});
  } else {
    store.removeAsiMod(m.id);
  }
}

async function trashDeployed(info: DeployedAsi) {
  const ok = await ask(
    `Remove ${info.name} from the game folder?\nIt'll be moved to modkit's trash (recoverable).`,
    { title: "Remove plugin", kind: "warning" }
  );
  if (ok) await store.trashDeployedAsi(info).catch(() => {});
}

async function adopt(info: DeployedAsi) {
  await store.adoptDeployedAsi(info).catch(() => {});
}

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

    <!-- Deployed in game (detected) -->
    <section v-if="gameInfo && gameInfo.deployed_asi.length" class="mt-6">
      <h3 class="mb-2 text-sm font-medium text-zinc-300">
        Deployed in game
        <span class="text-zinc-600">· {{ gameInfo.deployed_asi.length }} plugin{{ gameInfo.deployed_asi.length === 1 ? "" : "s" }}</span>
      </h3>
      <ul class="space-y-2">
        <li
          v-for="d in gameInfo.deployed_asi"
          :key="d.abs_path"
          class="flex items-center gap-3 rounded-xl border border-zinc-800 bg-zinc-900/40 px-4 py-2.5"
        >
          <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-violet-400" />
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span class="font-medium text-zinc-100">{{ d.name }}</span>
              <span
                v-if="d.known"
                class="rounded-full bg-sky-500/15 px-2 py-0.5 text-[11px] text-sky-300"
              >
                {{ d.known }}
              </span>
              <span
                v-if="store.isAsiManaged(d.name)"
                class="rounded-full bg-emerald-500/15 px-2 py-0.5 text-[11px] text-emerald-300"
              >
                managed
              </span>
            </div>
            <p class="truncate font-mono text-xs text-zinc-500">{{ d.rel_path }}</p>
          </div>
          <button
            v-if="!store.isAsiManaged(d.name)"
            class="shrink-0 rounded-md border border-zinc-700 px-2.5 py-1 text-xs text-zinc-300 hover:bg-zinc-800 disabled:opacity-40"
            :disabled="busy"
            title="Add this deployed plugin to the managed Library"
            @click="adopt(d)"
          >
            Adopt
          </button>
          <button
            class="shrink-0 rounded-md px-2.5 py-1 text-xs text-zinc-500 hover:bg-red-500/10 hover:text-red-400 disabled:opacity-40"
            :disabled="busy"
            title="Remove this plugin from the game folder (moved to trash)"
            @click="trashDeployed(d)"
          >
            Remove
          </button>
        </li>
      </ul>

      <p v-if="gameInfo.deployed_patches.length" class="mt-2 text-xs text-zinc-500">
        Patch WADs:
        <span class="font-mono text-zinc-400">{{ gameInfo.deployed_patches.join(", ") }}</span>
      </p>
    </section>

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
                class="rounded-full px-2 py-0.5 text-[11px]"
                :class="asiStatus(m).cls"
              >
                {{ asiStatus(m).label }}
              </span>
            </div>
            <p class="truncate text-xs text-zinc-500">
              v{{ m.version }} ·
              {{ m.asiFiles.length }} plugin{{ m.asiFiles.length === 1 ? "" : "s" }}
              ({{ m.asiFiles.join(", ") }})
            </p>
          </div>

          <!-- Deployed: offer redeploy + undeploy. Otherwise: deploy. -->
          <template v-if="store.isAsiDeployed(m)">
            <button
              class="rounded-md border border-zinc-700 px-2.5 py-1 text-xs text-zinc-300 hover:bg-zinc-800 disabled:opacity-40"
              :disabled="busy || !gameInfo || !store.isEnabled(m.id)"
              title="Copy the staged plugin over the deployed one again"
              @click="deploy(m)"
            >
              Redeploy
            </button>
            <button
              class="rounded-md border border-amber-600/40 px-2.5 py-1 text-xs text-amber-300 hover:bg-amber-500/10 disabled:opacity-40"
              :disabled="busy"
              title="Remove this plugin from the game folder (moved to trash)"
              @click="undeploy(m)"
            >
              Undeploy
            </button>
          </template>
          <button
            v-else
            class="rounded-md bg-emerald-600/90 px-2.5 py-1 text-xs font-medium text-white hover:bg-emerald-500 disabled:opacity-40"
            :disabled="busy || !gameInfo || !store.isEnabled(m.id)"
            :title="!gameInfo ? 'Set the game folder first' : !store.isEnabled(m.id) ? 'Enable it first' : ''"
            @click="deploy(m)"
          >
            Deploy
          </button>
          <button
            class="rounded-md px-2 py-1 text-xs text-zinc-500 hover:bg-red-500/10 hover:text-red-400 disabled:opacity-40"
            :disabled="busy"
            :title="store.isAsiDeployed(m) ? 'Undeploy (to trash) and forget from the Library' : 'Forget from the Library'"
            @click="removeFromLibrary(m)"
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
      v-if="!mods.length && !asiMods.length && !(gameInfo && gameInfo.deployed_asi.length)"
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
