<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { storeToRefs } from "pinia";
import { RouterLink } from "vue-router";
import { open, ask } from "@tauri-apps/plugin-dialog";
import { Switch } from "@headlessui/vue";
import { useProjectStore } from "../stores/project";
import type {
  AsiMod,
  DeployedAsi,
  LoadedMod,
  Origin,
  PrebuiltWad,
  ShipmentRef,
} from "../types";
import ConflictBadge from "../components/ConflictBadge.vue";
import ConfirmDialog from "../components/ConfirmDialog.vue";
import ProgressBar from "../components/ProgressBar.vue";

const store = useProjectStore();
const {
  mods,
  asiMods,
  shipments,
  prebuilt,
  wardrobe,
  textures,
  busy,
  error,
  conflictCount,
  gameInfo,
} = storeToRefs(store);

// Is the load order reflected in the deployed WAD? Drives the lane-1 state pills + footer.
const pending = computed(() => store.pending);

// Everything that composes into vz-patch.wad — the "into the patch WAD" lane.
const wadLaneCount = computed(
  () => shipments.value.length + mods.value.length + prebuilt.value.length,
);
// .asi found in the install that modkit doesn't manage — the adoptable rows in lane 2.
const unmanagedDetected = computed<DeployedAsi[]>(
  () => gameInfo.value?.deployed_asi.filter((d) => !store.isAsiManaged(d.name)) ?? [],
);

/** Where a mod came from, from its recorded origin. Never inferred from the page. */
function provenance(o?: Origin | null): { label: string; cls: string } {
  switch (o?.source) {
    case "registry":
      return { label: "mercs.ink", cls: "text-sky-300" };
    case "catalog":
      return { label: "repository", cls: "text-zinc-400" };
    case "imported":
      return { label: "imported", cls: "text-zinc-500" };
    default:
      return { label: "local", cls: "text-zinc-500" };
  }
}

/** State pill for a load-order item: the WAD as a whole is either staged or deployed. */
function wadState(enabled = true): { label: string; cls: string } {
  if (!enabled) return { label: "disabled", cls: "bg-zinc-600/25 text-zinc-400" };
  return pending.value.stale
    ? { label: "staged", cls: "bg-amber-500/15 text-amber-300" }
    : { label: "in game", cls: "bg-emerald-500/15 text-emerald-300" };
}

/** Deploy lifecycle status of a library ASI mod (lane 2). */
function asiStatus(m: AsiMod): { label: string; cls: string } {
  if (store.isAsiDeployed(m)) return { label: "deployed", cls: "bg-emerald-500/15 text-emerald-300" };
  if (!store.isEnabled(m.id)) return { label: "disabled", cls: "bg-zinc-600/25 text-zinc-400" };
  if (!gameInfo.value) return { label: "enabled · no game", cls: "bg-amber-500/15 text-amber-300" };
  return { label: "ready to deploy", cls: "bg-sky-500/15 text-sky-300" };
}

async function undeploy(m: AsiMod) {
  const ok = await ask(
    `Remove ${m.name}'s plugin(s) from the game folder?\nThey'll be moved to modkit's trash (recoverable).`,
    { title: "Undeploy", kind: "warning" },
  );
  if (ok) await store.undeployAsiMod(m).catch(() => {});
}

// --- Removal: gated behind a confirmation modal that disables the mod (and its dependents) first ---
type PendingRemoval =
  | { kind: "asi"; id: string; name: string; mod: AsiMod }
  | { kind: "wad"; id: string; name: string };

const pendingRemoval = ref<PendingRemoval | null>(null);

const removalDependents = computed<LoadedMod[]>(() =>
  pendingRemoval.value ? store.dependentsOf(pendingRemoval.value.name) : [],
);

function requestRemoveAsi(m: AsiMod) {
  pendingRemoval.value = { kind: "asi", id: m.id, name: m.name, mod: m };
}
function requestRemoveWad(m: LoadedMod) {
  pendingRemoval.value = { kind: "wad", id: m.id, name: m.manifest.name };
}

async function confirmRemoval() {
  const p = pendingRemoval.value;
  if (!p) return;
  store.setModEnabled(p.id, false);
  for (const dep of removalDependents.value) store.setModEnabled(dep.id, false);
  if (p.kind === "asi") {
    await store.forceRemoveAsiMod(p.mod).catch(() => {});
  } else {
    store.removeMod(p.id);
  }
  pendingRemoval.value = null;
}

function removeShipment(s: ShipmentRef) {
  store.removeShipment(s.id);
}
function removePrebuilt(p: PrebuiltWad) {
  store.removePrebuilt(p.id);
}

async function trashDeployed(info: DeployedAsi) {
  const ok = await ask(
    `Remove ${info.name} from the game folder?\nIt'll be moved to modkit's trash (recoverable).`,
    { title: "Remove plugin", kind: "warning" },
  );
  if (ok) await store.trashDeployedAsi(info).catch(() => {});
}

async function adopt(info: DeployedAsi) {
  await store.adoptDeployedAsi(info).catch(() => {});
}
async function updateMod(m: AsiMod) {
  await store.updateAsiMod(m).catch(() => {});
}

onMounted(() => {
  if (store.catalog.length === 0) store.fetchCatalog();
});

const ASI_TARGETS = [
  { value: "scripts", label: "scripts/" },
  { value: ".", label: "game root" },
  { value: "plugins", label: "plugins/" },
  { value: "update", label: "update/" },
];

async function addMod() {
  const dir = await open({ directory: true, title: "Select a mod folder" });
  if (typeof dir === "string") await store.loadModFromDir(dir).catch(() => {});
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
async function addWad() {
  const f = await open({
    title: "Select a mod's vz-patch.wad",
    filters: [{ name: "Patch WAD", extensions: ["wad"] }],
  });
  if (typeof f === "string") await store.importPatchWad(f).catch(() => {});
}
async function deploy(mod: AsiMod) {
  await store.deployAsiMod(mod).catch(() => {});
}
async function deployEnabled() {
  for (const m of asiMods.value) {
    if (store.isEnabled(m.id)) await store.deployAsiMod(m).catch(() => {});
  }
}

const nothingHere = computed(
  () =>
    wadLaneCount.value === 0 &&
    asiMods.value.length === 0 &&
    wardrobe.value.length === 0 &&
    textures.value.length === 0 &&
    unmanagedDetected.value.length === 0,
);
</script>

<template>
  <div class="mx-auto max-w-4xl px-8 py-6">
    <header class="flex items-center justify-between gap-4">
      <div>
        <h2 class="plate-title text-xl">Mod Library</h2>
        <p class="text-sm text-zinc-500">
          {{ wadLaneCount }} in the patch WAD ·
          {{ asiMods.length }} plugin{{ asiMods.length === 1 ? "" : "s" }}
        </p>
      </div>
      <div class="flex flex-wrap items-center justify-end gap-2">
        <ConflictBadge v-if="mods.length" :count="conflictCount" />
        <RouterLink to="/catalog" class="btn-outline">Mod Market</RouterLink>
        <button class="btn-outline" :disabled="busy" @click="addPlugin">Add plugin</button>
        <button class="btn-outline" :disabled="busy" @click="addWad">Add WAD</button>
        <button class="btn-plate" :disabled="busy" @click="addMod">Add folder</button>
      </div>
    </header>

    <ProgressBar v-if="busy" indeterminate class="mt-4" />

    <div
      v-if="error"
      class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
    >
      {{ error }}
    </div>

    <!-- ═══════════════ LANE 1 — into the patch WAD (the load order) ═══════════════ -->
    <section v-if="wadLaneCount || wardrobe.length || textures.length" class="mt-7">
      <div class="mb-1 flex items-baseline gap-3">
        <h3 class="plate-label">Into the patch WAD</h3>
        <span class="text-xs text-zinc-600"
          >the load order · composed into <code class="text-zinc-500">vz-patch.wad</code></span
        >
      </div>

      <ul class="mt-3 space-y-2">
        <!-- Shipments (mercs.ink + Workshop + local): qm source, built into the WAD. -->
        <li
          v-for="s in shipments"
          :key="s.id"
          class="engraved flex items-center gap-3 rounded-xl border border-zinc-800 bg-zinc-900/50 px-4 py-3"
        >
          <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-sky-400" />
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-2">
              <span class="font-medium text-zinc-100">{{ s.name }}</span>
              <span class="stamp" :class="provenance(s.origin).cls">{{ provenance(s.origin).label }}</span>
              <span class="stamp text-zinc-300">Shipment</span>
            </div>
            <p class="truncate font-mono text-xs text-zinc-500">
              <span v-if="s.version">v{{ s.version }} · </span>{{ s.slug ?? s.name }}
            </p>
          </div>
          <span class="rounded-full px-2 py-0.5 text-[11px]" :class="wadState().cls">{{ wadState().label }}</span>
          <button
            class="rounded-md px-2 py-1 text-xs text-zinc-400 hover:bg-red-500/10 hover:text-red-400"
            title="Remove this Shipment from the load order"
            @click="removeShipment(s)"
          >
            Remove
          </button>
        </li>

        <!-- WAD-asset mods: raw assets, ordered (later wins conflicts). -->
        <li
          v-for="(m, i) in mods"
          :key="m.id"
          class="engraved flex items-center gap-3 rounded-xl border border-zinc-800 bg-zinc-900/50 px-4 py-3"
        >
          <div class="flex flex-col text-zinc-600">
            <button
              class="hover:text-zinc-300 disabled:opacity-30"
              :disabled="i === 0"
              title="Load earlier"
              @click="store.moveMod(m.id, 'up')"
            >
              ▲
            </button>
            <button
              class="hover:text-zinc-300 disabled:opacity-30"
              :disabled="i === mods.length - 1"
              title="Load later (wins conflicts)"
              @click="store.moveMod(m.id, 'down')"
            >
              ▼
            </button>
          </div>
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
          <div class="min-w-0 flex-1" :class="{ 'opacity-50': !store.isEnabled(m.id) }">
            <div class="flex flex-wrap items-center gap-2">
              <RouterLink
                :to="`/mod/${m.id}`"
                class="font-medium text-zinc-100 hover:text-emerald-400"
                >{{ m.manifest.name }}</RouterLink
              >
              <span class="stamp" :class="provenance(m.origin).cls">{{ provenance(m.origin).label }}</span>
              <span class="stamp text-zinc-300">WAD mod</span>
            </div>
            <p class="truncate text-xs text-zinc-500">
              v{{ m.manifest.version }}<span v-if="m.manifest.author"> · {{ m.manifest.author }}</span> ·
              {{ m.assets.length }} asset{{ m.assets.length === 1 ? "" : "s" }}
            </p>
          </div>
          <span class="rounded-full px-2 py-0.5 text-[11px]" :class="wadState(store.isEnabled(m.id)).cls">{{
            wadState(store.isEnabled(m.id)).label
          }}</span>
          <button
            class="rounded-md px-2 py-1 text-xs text-zinc-400 hover:bg-red-500/10 hover:text-red-400"
            @click="requestRemoveWad(m)"
          >
            Remove
          </button>
        </li>

        <!-- Imported patch WADs: finished WADs, merged into the one the game loads. -->
        <li
          v-for="(p, i) in prebuilt"
          :key="p.id"
          class="engraved flex items-center gap-3 rounded-xl border border-zinc-800 bg-zinc-900/50 px-4 py-3"
        >
          <div class="flex flex-col text-zinc-600">
            <button
              class="hover:text-zinc-300 disabled:opacity-30"
              :disabled="i === 0"
              title="Merge earlier"
              @click="store.movePrebuilt(p.id, 'up')"
            >
              ▲
            </button>
            <button
              class="hover:text-zinc-300 disabled:opacity-30"
              :disabled="i === prebuilt.length - 1"
              title="Merge later (wins)"
              @click="store.movePrebuilt(p.id, 'down')"
            >
              ▼
            </button>
          </div>
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-2">
              <span class="font-medium text-zinc-100">{{ p.name }}</span>
              <span class="stamp text-zinc-500">imported</span>
              <span class="stamp text-zinc-300">patch WAD</span>
            </div>
            <p class="truncate text-xs text-zinc-500">
              {{ p.asset_count }} asset{{ p.asset_count === 1 ? "" : "s" }} ·
              {{ p.block_count }} block{{ p.block_count === 1 ? "" : "s" }}
            </p>
          </div>
          <span class="rounded-full px-2 py-0.5 text-[11px]" :class="wadState().cls">{{ wadState().label }}</span>
          <button
            class="rounded-md px-2 py-1 text-xs text-zinc-400 hover:bg-red-500/10 hover:text-red-400"
            @click="removePrebuilt(p)"
          >
            Remove
          </button>
        </li>

        <!-- Additions: wardrobe outfits + texture swaps, built into the same WAD. -->
        <li
          v-if="wardrobe.length || textures.length"
          class="flex items-center gap-3 rounded-xl border border-dashed border-zinc-800 bg-zinc-900/30 px-4 py-2.5"
        >
          <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-zinc-600" />
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-2">
              <span class="font-medium text-zinc-300">Additions</span>
              <span class="stamp text-zinc-500">wardrobe · textures</span>
            </div>
            <p class="text-xs text-zinc-500">
              <span v-if="wardrobe.length">{{ wardrobe.length }} outfit{{ wardrobe.length === 1 ? "" : "s" }}</span>
              <span v-if="wardrobe.length && textures.length"> · </span>
              <span v-if="textures.length">{{ textures.length }} texture swap{{ textures.length === 1 ? "" : "s" }}</span>
              — built into the same WAD
            </p>
          </div>
          <RouterLink to="/wardrobe" class="btn-outline shrink-0">Wardrobe</RouterLink>
          <RouterLink to="/textures" class="btn-outline shrink-0">Textures</RouterLink>
        </li>
      </ul>

      <p
        v-if="wadLaneCount || wardrobe.length || textures.length"
        class="mt-3 flex items-center gap-2 text-xs"
        :class="pending.stale ? 'text-amber-300' : 'text-emerald-300/90'"
      >
        <span
          class="h-1.5 w-1.5 rounded-full"
          :class="pending.stale ? 'bg-amber-400' : 'bg-emerald-400'"
        />
        <template v-if="pending.stale"
          >{{ pending.count }} change{{ pending.count === 1 ? "" : "s" }} staged — not in your game
          yet. Press <strong class="text-amber-200">▶ Apply &amp; Play</strong> in the game bar.</template
        >
        <template v-else>This load order is in your game.</template>
      </p>
    </section>

    <!-- ═══════════════ LANE 2 — into the game folder (native plugins) ═══════════════ -->
    <section v-if="asiMods.length || unmanagedDetected.length" class="mt-8">
      <div class="mb-1 flex flex-wrap items-center justify-between gap-2">
        <div class="flex items-baseline gap-3">
          <h3 class="plate-label">Into the game folder</h3>
          <span class="text-xs text-zinc-600">native <code class="text-zinc-500">.asi</code> plugins · copied beside the exe</span>
        </div>
        <div v-if="asiMods.length" class="flex items-center gap-2 text-xs">
          <label class="text-zinc-500">Deploy to</label>
          <select
            :value="store.asiTarget"
            class="field px-2 py-1"
            @change="store.setAsiTarget(($event.target as HTMLSelectElement).value)"
          >
            <option v-for="t in ASI_TARGETS" :key="t.value" :value="t.value">{{ t.label }}</option>
          </select>
          <button
            class="btn-plate"
            :disabled="busy || !gameInfo"
            :title="!gameInfo ? 'Set the game folder first' : ''"
            @click="deployEnabled"
          >
            Deploy enabled
          </button>
        </div>
      </div>

      <ul class="mt-3 space-y-2">
        <!-- Managed ASI plugins. -->
        <li
          v-for="m in asiMods"
          :key="m.id"
          class="engraved flex items-center gap-3 rounded-xl border border-zinc-800 bg-zinc-900/50 px-4 py-3"
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
          <div class="min-w-0 flex-1" :class="{ 'opacity-50': !store.isEnabled(m.id) }">
            <div class="flex flex-wrap items-center gap-2">
              <span class="font-medium text-zinc-100">{{ m.name }}</span>
              <span class="stamp" :class="provenance(m.origin).cls">{{ provenance(m.origin).label }}</span>
              <span class="stamp text-zinc-300">ASI plugin</span>
            </div>
            <p class="truncate text-xs text-zinc-500">
              v{{ m.version }} ·
              {{ m.asiFiles.length }} plugin{{ m.asiFiles.length === 1 ? "" : "s" }}
              ({{ m.asiFiles.join(", ") }})
            </p>
          </div>
          <span class="rounded-full px-2 py-0.5 text-[11px]" :class="asiStatus(m).cls">{{ asiStatus(m).label }}</span>
          <button
            v-if="store.asiUpdate(m)"
            class="rounded-md bg-sky-600 px-2.5 py-1 text-xs font-medium text-white hover:bg-sky-500 disabled:opacity-40"
            :disabled="busy"
            :title="`Update to v${store.asiUpdate(m)?.version} and redeploy if deployed`"
            @click="updateMod(m)"
          >
            Update → v{{ store.asiUpdate(m)?.version }}
          </button>
          <template v-if="store.isAsiDeployed(m)">
            <button
              class="btn-outline"
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
            class="btn-plate"
            :disabled="busy || !gameInfo || !store.isEnabled(m.id)"
            :title="!gameInfo ? 'Set the game folder first' : !store.isEnabled(m.id) ? 'Enable it first' : ''"
            @click="deploy(m)"
          >
            Deploy
          </button>
          <button
            class="rounded-md px-2 py-1 text-xs text-zinc-400 hover:bg-red-500/10 hover:text-red-400"
            :title="store.isAsiDeployed(m) ? 'Undeploy (to trash) and forget from the Library' : 'Forget from the Library'"
            @click="requestRemoveAsi(m)"
          >
            Remove
          </button>
        </li>

        <!-- Unmanaged plugins found in the install: offer to adopt them. -->
        <li
          v-for="d in unmanagedDetected"
          :key="d.abs_path"
          class="engraved flex items-center gap-3 rounded-xl border border-dashed border-zinc-800 bg-zinc-900/30 px-4 py-2.5"
        >
          <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-violet-400" />
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-2">
              <span class="font-medium text-zinc-100">{{ d.name }}</span>
              <span v-if="d.known" class="stamp text-sky-300">{{ d.known }}</span>
              <span class="stamp text-zinc-300">ASI plugin</span>
            </div>
            <p class="truncate font-mono text-xs text-zinc-500">
              {{ d.rel_path }} — found in your install, not managed
            </p>
          </div>
          <span class="rounded-full px-2 py-0.5 text-[11px] text-zinc-400 border border-zinc-700">unmanaged</span>
          <button
            class="btn-outline shrink-0"
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

      <p v-if="gameInfo && gameInfo.deployed_patches.length" class="mt-2 text-xs text-zinc-600">
        Patch WAD in the game folder:
        <span class="font-mono text-zinc-500">{{ gameInfo.deployed_patches.join(", ") }}</span>
      </p>
    </section>

    <!-- Empty -->
    <div v-if="nothingHere" class="empty-plate mt-10">
      <p class="text-zinc-400">No mods yet.</p>
      <p class="mt-1 text-sm text-zinc-600">
        <RouterLink to="/catalog" class="text-emerald-400 hover:underline">Browse the Mod Market</RouterLink>
        for mercs.ink Shipments and repository plugins, or add a folder, a
        <code class="text-zinc-400">.asi</code> plugin, or a
        <code class="text-zinc-400">vz-patch.wad</code>.
      </p>
    </div>

    <!-- Removal confirmation: disables the mod (and its dependents) first -->
    <ConfirmDialog
      :open="!!pendingRemoval"
      :title="`Remove ${pendingRemoval?.name ?? ''}?`"
      confirm-label="Disable & remove"
      danger
      @confirm="confirmRemoval"
      @cancel="pendingRemoval = null"
    >
      <p>
        This will disable <strong class="text-zinc-200">{{ pendingRemoval?.name }}</strong>
        <template v-if="pendingRemoval?.kind === 'asi'">
          and remove its plugin file(s) from the game folder (moved to modkit's recoverable trash).
        </template>
        <template v-else> and remove it from the Library.</template>
      </p>
      <div
        v-if="removalDependents.length"
        class="mt-3 rounded-lg border border-amber-600/30 bg-amber-500/10 px-3 py-2 text-amber-200"
      >
        <p class="font-medium">
          {{ removalDependents.length }} mod{{ removalDependents.length === 1 ? "" : "s" }}
          depend on it and will be disabled too:
        </p>
        <ul class="mt-1 list-inside list-disc">
          <li v-for="d in removalDependents" :key="d.id">{{ d.manifest.name }}</li>
        </ul>
      </div>
    </ConfirmDialog>
  </div>
</template>
