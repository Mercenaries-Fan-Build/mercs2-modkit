<script setup lang="ts">
import { ref, computed, onMounted } from "vue";
import { storeToRefs } from "pinia";
import { open } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "../stores/project";
import ProgressBar from "../components/ProgressBar.vue";
import type { ClaimConflict, GroupOutcome } from "../types";

const store = useProjectStore();
const {
  busy,
  error,
  buildResult,
  validation,
  gameInfo,
  wadBackups,
  wardrobe,
  prebuilt,
  shipments,
  textures,
} = storeToRefs(store);

async function importWad() {
  const f = await open({
    title: "Select a mod's vz-patch.wad",
    filters: [{ name: "Patch WAD", extensions: ["wad"] }],
  });
  if (typeof f === "string") await store.importPatchWad(f).catch(() => {});
}

const nothingToBuild = computed(
  () =>
    store.enabledMods.length === 0 &&
    wardrobe.value.length === 0 &&
    prebuilt.value.length === 0 &&
    shipments.value.length === 0 &&
    textures.value.length === 0,
);

const simulatorPath = ref<string | null>(null);
const stage = ref("");
// Partial overlaps between mods: unresolvable automatically, so we show them and stop.
const conflicts = ref<ClaimConflict[]>([]);
const deployed = ref<string | null>(null);

onMounted(() => void store.loadWadBackups().catch(() => {}));

/** Build into the managed staging dir, then validate with wad_simulator. */
async function buildAndValidate() {
  store.error = null;
  conflicts.value = [];
  deployed.value = null;

  stage.value = "Resolving load order…";
  try {
    await store.previewConflicts();
  } catch (e: unknown) {
    // The backend rejects a partial overlap rather than shipping a half-applied mod.
    const payload = e as { conflicts?: ClaimConflict[] };
    if (payload?.conflicts?.length) {
      conflicts.value = payload.conflicts;
      stage.value = "";
      return;
    }
    store.error = String(e);
    stage.value = "";
    return;
  }

  stage.value = "Assembling patch WAD…";
  const result = await store.assemble({}).catch(() => null);
  if (!result) {
    stage.value = "";
    return;
  }

  stage.value = "Validating with wad_simulator…";
  try {
    let sim = simulatorPath.value;
    if (!sim) {
      sim = await store.fetchSimulator().catch(() => null);
      if (sim) simulatorPath.value = sim;
    }
    await store.validate(result.path, sim);
  } catch {
    /* surfaced via store.error */
  } finally {
    stage.value = "";
  }
}

/** Install the built WAD. The previous one is snapshotted first — this is undoable. */
async function deploy() {
  if (!buildResult.value) return;
  const res = await store.deployPatchWad(buildResult.value.path).catch(() => null);
  if (res) deployed.value = res.installed_at;
}

async function restore(file: string | null) {
  await store.restorePatchWad(file).catch(() => {});
  deployed.value = null;
}

function fmtBytes(n: number): string {
  if (n > 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MB`;
  if (n > 1 << 10) return `${(n / (1 << 10)).toFixed(1)} KB`;
  return `${n} B`;
}

function outcomeText(o: GroupOutcome): string {
  switch (o.outcome) {
    case "applied":
      return `${o.asset_count} asset${o.asset_count === 1 ? "" : "s"} applied`;
    case "overridden":
      return `fully overridden by “${o.overridden_by_label}”`;
    case "partially_applied":
      return `${o.applied} applied, ${o.overridden} overridden by a later mod`;
  }
}
</script>

<template>
  <div class="mx-auto max-w-3xl px-8 py-6">
    <header>
      <h2 class="plate-title text-xl">Build &amp; Deploy</h2>
      <p class="text-sm text-zinc-500">
        Assemble your mods into one <code>vz-patch.wad</code>, check it, then install it.
      </p>
    </header>

    <div
      v-if="nothingToBuild"
      class="empty-plate mt-10"
    >
      Nothing to build yet. Enable a mod, add a
      <RouterLink to="/wardrobe" class="text-emerald-400 underline">wardrobe outfit</RouterLink>
      or a
      <RouterLink to="/textures" class="text-emerald-400 underline">texture</RouterLink>,
      or add an existing mod WAD below.
      <div class="mt-4">
        <button class="btn-secondary" @click="importWad">Add a WAD…</button>
      </div>
    </div>

    <template v-else>
      <!-- Wardrobe outfits and texture swaps build into the same WAD as the mods. -->
      <div
        v-if="wardrobe.length || textures.length"
        class="engraved mt-4 rounded-lg border border-zinc-800 bg-zinc-900/50 px-4 py-3 text-sm text-zinc-400"
      >
        <span v-if="wardrobe.length">
          {{ wardrobe.length }} wardrobe outfit{{ wardrobe.length === 1 ? "" : "s" }}
        </span>
        <span v-if="wardrobe.length && textures.length"> and </span>
        <span v-if="textures.length">
          {{ textures.length }} texture{{ textures.length === 1 ? "" : "s" }}
        </span>
        will be included.
      </div>
      <!-- Imported community WADs. The game loads only one patch WAD, so installing two
           prebuilt mods has never been possible — modkit merges them into one. -->
      <section class="guilloche mt-6 rounded-xl border border-zinc-800 p-5">
        <div class="flex items-center justify-between">
          <div>
            <h3 class="plate-label">Mod WADs</h3>
            <p class="mt-1 text-xs text-zinc-500">
              Add existing <code>vz-patch.wad</code> mods. Normally you could only use one at
              a time — these get merged together.
            </p>
          </div>
          <button class="btn-secondary shrink-0" @click="importWad">Add a WAD…</button>
        </div>

        <ul v-if="prebuilt.length" class="mt-4 space-y-2">
          <li
            v-for="(p, i) in prebuilt"
            :key="p.id"
            class="engraved rounded-lg border border-zinc-800 bg-zinc-900/50 px-3 py-2"
          >
            <div class="flex items-center gap-3">
              <span class="w-6 text-right text-xs text-zinc-600">{{ i + 1 }}</span>
              <div class="min-w-0 flex-1">
                <p class="truncate text-sm text-zinc-200">{{ p.name }}</p>
                <p class="text-xs text-zinc-500">
                  {{ p.asset_count }} asset{{ p.asset_count === 1 ? "" : "s" }} ·
                  {{ p.block_count }} block{{ p.block_count === 1 ? "" : "s" }}
                </p>
              </div>
              <button
                class="btn-secondary px-2 py-1"
                :disabled="i === 0"
                @click="store.movePrebuilt(p.id, 'up')"
              >
                ↑
              </button>
              <button
                class="btn-secondary px-2 py-1"
                :disabled="i === prebuilt.length - 1"
                @click="store.movePrebuilt(p.id, 'down')"
              >
                ↓
              </button>
              <button class="btn-secondary px-2 py-1" @click="store.removePrebuilt(p.id)">
                ✕
              </button>
            </div>
            <p
              v-for="(w, wi) in p.warnings"
              :key="wi"
              class="mt-2 rounded border border-amber-500/30 bg-amber-500/10 px-2 py-1 text-xs text-amber-300"
            >
              {{ w }}
            </p>
          </li>
        </ul>
        <p v-else class="mt-3 text-xs text-zinc-600">None added.</p>
      </section>

      <!-- Shipments handed over from the Workshop's "Send to Modkit". Source projects, not finished
           WADs: modkit builds and Lua-links them through Quartermaster at build time, so several
           script-touching Shipments reconcile instead of one clobbering another. -->
      <section
        v-if="shipments.length"
        class="guilloche mt-6 rounded-xl border border-zinc-800 p-5"
      >
        <div>
          <h3 class="plate-label">Workshop Shipments</h3>
          <p class="mt-1 text-xs text-zinc-500">
            Sent from the Workshop. Built and script-linked through Quartermaster when you build
            below — no need to add them by hand.
          </p>
        </div>
        <ul class="mt-4 space-y-2">
          <li
            v-for="(s, i) in shipments"
            :key="s.id"
            class="engraved flex items-center gap-3 rounded-lg border border-zinc-800 bg-zinc-900/50 px-3 py-2"
          >
            <span class="w-6 text-right text-xs text-zinc-600">{{ i + 1 }}</span>
            <div class="min-w-0 flex-1">
              <p class="truncate text-sm text-zinc-200">{{ s.name }}</p>
              <p class="truncate font-mono text-xs text-zinc-500">{{ s.path }}</p>
            </div>
            <button class="btn-secondary px-2 py-1" @click="store.removeShipment(s.id)">
              ✕
            </button>
          </li>
        </ul>
      </section>

      <!-- Load order. Later mods override earlier ones — same rule as the engine. -->
      <section
        v-if="store.enabledMods.length"
        class="guilloche mt-6 rounded-xl border border-zinc-800 p-5"
      >
        <h3 class="plate-label">Load order</h3>
        <p class="mt-1 text-xs text-zinc-500">
          Later mods override earlier ones. If two mods change the same thing, the one
          lower in this list wins.
        </p>

        <ul class="mt-4 space-y-2">
          <li
            v-for="(m, i) in store.enabledMods"
            :key="m.id"
            class="engraved flex items-center gap-3 rounded-lg border border-zinc-800 bg-zinc-900/50 px-3 py-2"
          >
            <span class="w-6 text-right text-xs text-zinc-600">{{ i + 1 }}</span>
            <div class="min-w-0 flex-1">
              <p class="truncate text-sm text-zinc-200">{{ m.manifest.name }}</p>
              <p class="text-xs text-zinc-500">
                {{ m.assets.length }} asset{{ m.assets.length === 1 ? "" : "s" }}
              </p>
            </div>
            <!-- Buttons, not drag-and-drop: the whole UI is gamepad-navigable and
                 HTML5 drag targets are unreachable with a controller. -->
            <button
              class="btn-secondary px-2 py-1"
              :disabled="i === 0"
              title="Load earlier (overridden by more mods)"
              @click="store.moveMod(m.id, 'up')"
            >
              ↑
            </button>
            <button
              class="btn-secondary px-2 py-1"
              :disabled="i === store.enabledMods.length - 1"
              title="Load later (overrides more mods)"
              @click="store.moveMod(m.id, 'down')"
            >
              ↓
            </button>
          </li>
        </ul>
        <p class="mt-3 text-xs text-zinc-600">
          ↑ loads earlier · ↓ loads later (wins)
        </p>
      </section>

      <button
        class="btn-plate mt-5 w-full justify-center"
        :disabled="busy"
        @click="buildAndValidate"
      >
        Build &amp; Check
      </button>

      <ProgressBar v-if="busy" indeterminate :label="stage" class="mt-4" />

      <!-- Unresolvable overlap: two mods change an overlapping-but-different set of
           assets. Picking per-asset would produce a mod nobody authored, so we stop. -->
      <section v-if="conflicts.length" class="mt-6 space-y-3">
        <div
          v-for="(c, i) in conflicts"
          :key="i"
          class="rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-200"
        >
          <p class="font-medium">Can’t combine these two mods automatically</p>
          <p class="mt-1 text-red-300/90">{{ c.message }}</p>
        </div>
      </section>

      <div
        v-if="error"
        class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
      >
        {{ error }}
      </div>

      <!-- What the load order actually did. -->
      <section v-if="buildResult" class="mt-6">
        <h3 class="plate-label mb-2">Result</h3>
        <div class="engraved rounded-lg border border-zinc-800 bg-zinc-900/50 px-4 py-3 text-sm">
          <p class="font-mono text-xs break-all text-zinc-200">{{ buildResult.path }}</p>
          <p class="mt-1 text-xs text-zinc-500">
            {{ buildResult.block_count }} block{{ buildResult.block_count === 1 ? "" : "s" }}
            · {{ fmtBytes(buildResult.byte_size) }}
          </p>
          <p class="mt-1 font-mono text-[11px] text-zinc-600">
            sha256 {{ buildResult.sha256.slice(0, 32) }}…
          </p>
        </div>

        <p
          v-for="(w, i) in buildResult.warnings ?? []"
          :key="`warn-${i}`"
          class="mt-2 rounded border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-300"
        >
          {{ w }}
        </p>

        <ul class="mt-3 space-y-1">
          <li
            v-for="(o, i) in buildResult.outcomes"
            :key="i"
            class="flex items-center gap-2 text-xs"
          >
            <span
              class="stamp"
              :class="
                o.outcome === 'applied'
                  ? 'text-emerald-300'
                  : 'text-amber-300'
              "
            >
              {{ o.outcome === "applied" ? "applied" : "overridden" }}
            </span>
            <span class="text-zinc-300">{{ o.label }}</span>
            <span class="text-zinc-600">— {{ outcomeText(o) }}</span>
          </li>
        </ul>
      </section>

      <!-- Validation -->
      <section v-if="validation" class="mt-6">
        <h3 class="plate-label mb-2 flex items-center gap-2">
          Validation
          <span
            class="stamp"
            :class="
              validation.ok
                ? 'text-emerald-300'
                : 'text-red-300'
            "
          >
            {{ validation.ok ? "passed" : "failed" }}
            (exit {{ validation.exit_code ?? "?" }})
          </span>
        </h3>
        <pre
          class="max-h-60 overflow-auto rounded-lg border border-zinc-800 bg-black/40 p-3 text-xs text-zinc-400"
        >{{ validation.stdout || validation.stderr || "(no output)" }}</pre>
      </section>

      <!-- Install. Close the game first: it holds the WAD open. -->
      <section v-if="buildResult" class="guilloche mt-6 rounded-xl border border-zinc-800 p-5">
        <h3 class="plate-label">Install</h3>
        <p class="mt-1 text-xs text-zinc-500">
          Copies the WAD into
          <code>{{ gameInfo?.data_dir ?? "the game's data folder" }}</code
          >. Your current <code>vz-patch.wad</code> is backed up first, so you can undo
          this. <strong class="text-zinc-400">Close the game before installing.</strong>
        </p>
        <button
          class="btn-plate mt-3 w-full justify-center"
          :disabled="busy || !gameInfo?.data_dir"
          @click="deploy"
        >
          Install into the game
        </button>
        <p
          v-if="deployed"
          class="mt-3 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-300"
        >
          Installed to <span class="font-mono">{{ deployed }}</span
          >. Launch the game, then check Diagnostics if anything looks wrong.
        </p>
      </section>

      <!-- Undo. Every hazard a bad WAD can cause is recoverable from here. -->
      <section class="guilloche mt-6 rounded-xl border border-zinc-800 p-5">
        <h3 class="plate-label">Undo</h3>
        <p class="mt-1 text-xs text-zinc-500">
          Restore a previous patch, or remove it entirely to go back to the unmodded game.
        </p>
        <button
          class="btn-outline mt-3 w-full justify-center"
          :disabled="busy || !gameInfo?.data_dir"
          @click="restore(null)"
        >
          Remove the patch (back to stock game)
        </button>

        <ul v-if="wadBackups.length" class="mt-3 space-y-2">
          <li
            v-for="b in wadBackups"
            :key="b.file"
            class="engraved flex items-center gap-3 rounded-lg border border-zinc-800 bg-zinc-900/50 px-3 py-2"
          >
            <div class="min-w-0 flex-1">
              <p class="truncate font-mono text-xs text-zinc-300">
                {{ b.sha256.slice(0, 16) }}…
              </p>
              <p class="text-xs text-zinc-500">{{ fmtBytes(b.byte_size) }}</p>
            </div>
            <button
              class="btn-secondary"
              :disabled="busy || !gameInfo?.data_dir"
              @click="restore(b.file)"
            >
              Restore
            </button>
          </li>
        </ul>
      </section>
    </template>
  </div>
</template>

<style scoped>
/* Was hardcoded rgb(), so it kept the stock palette after the retint. Points
   at the tokens now, and matches the shared `.btn-outline` treatment. */
.btn-secondary {
  display: inline-flex;
  align-items: center;
  gap: 0.5rem;
  border-radius: var(--radius-lg);
  border: 1px solid var(--color-zinc-700);
  padding: 0.375rem 0.875rem;
  font-size: 0.75rem;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--color-zinc-300);
  transition: border-color 0.15s, color 0.15s, background-color 0.15s;
}
.btn-secondary:hover:not(:disabled) {
  border-color: var(--color-brass-700);
  color: var(--color-brass-300);
  background-color: rgb(203 176 66 / 0.06);
}
.btn-secondary:disabled {
  opacity: 0.45;
}
</style>
