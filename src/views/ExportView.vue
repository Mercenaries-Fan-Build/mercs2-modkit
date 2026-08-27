<script setup lang="ts">
import { ref, computed, onMounted, nextTick } from "vue";
import { storeToRefs } from "pinia";
import { useRoute } from "vue-router";
import { open } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "../stores/project";
import ProgressBar from "../components/ProgressBar.vue";
import type { ClaimConflict, GroupOutcome, PlacementOutcome } from "../types";

const route = useRoute();
// The validation report, and a transient highlight when arrived at via the game bar's
// "view validation report" link (`/export?show=validation`).
const validationSection = ref<HTMLElement | null>(null);
const highlightValidation = ref(false);
async function focusValidation() {
  await nextTick();
  const el = validationSection.value;
  if (!el) return;
  el.scrollIntoView({ behavior: "smooth", block: "center" });
  highlightValidation.value = true;
  setTimeout(() => (highlightValidation.value = false), 2400);
}

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

async function addShipment() {
  // Stage a Quartermaster SOURCE folder, not a finished WAD: qm builds and Lua-links the whole
  // staged set at assemble time, so several script-touching mods reconcile into one scripts_vz
  // instead of one clobbering another — the overlap a finished .wad import cannot merge past.
  const dir = await open({ directory: true, title: "Select a Shipment folder" });
  if (typeof dir === "string") await store.importShipment(dir).catch(() => {});
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
// The loose-file half of the last install/uninstall — what landed in the game folder, and what
// came back out. Shown because a file dropped into the game install is not visible anywhere else.
const placement = ref<PlacementOutcome | null>(null);

onMounted(() => {
  void store.loadWadBackups().catch(() => {});
  // Deep link from the game bar: land on the validation report and flag it.
  if (route.query.show === "validation" && store.validation) void focusValidation();
});

/** Build into the managed staging dir, then validate with wad_simulator. */
async function buildAndValidate() {
  store.error = null;
  conflicts.value = [];
  deployed.value = null;
  placement.value = null;

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

  // A Shipment carrying only native_hook / place_file contributions builds no WAD at all, and
  // there is nothing for the simulator to load. Its files are still a complete, installable build.
  if (!result.path) {
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

/** Install the build. The previous WAD is snapshotted first — this is undoable. */
async function deploy() {
  if (!buildResult.value) return;
  const res = await store.deployPatchWad(buildResult.value).catch(() => null);
  if (!res) return;
  deployed.value = res.installed_at;
  placement.value = res.files;
}

async function restore(file: string | null) {
  const res = await store.restorePatchWad(file).catch(() => null);
  deployed.value = null;
  placement.value = res?.files ?? null;
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
      or add a Shipment below.
      <div class="mt-4">
        <button class="btn-secondary" @click="addShipment">Add Shipment…</button>
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
      <!-- Imported community WADs (legacy). New mods are added as Shipments below; this section
           only appears when finished WADs are already in the load order, so they stay reorderable
           and removable. The game loads one patch WAD, so these are still merged into it. -->
      <section v-if="prebuilt.length" class="guilloche mt-6 rounded-xl border border-zinc-800 p-5">
        <div>
          <h3 class="plate-label">Mod WADs</h3>
          <p class="mt-1 text-xs text-zinc-500">
            Finished <code>vz-patch.wad</code> mods already in the load order — merged into the one
            the game loads. Two that partially overlap can't be reconciled here; add mods as
            Shipments instead.
          </p>
        </div>

        <ul class="mt-4 space-y-2">
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
      </section>

      <!-- Shipments: Quartermaster source projects, not finished WADs. modkit builds and Lua-links
           the whole set through qm at build time, so several script-touching Shipments reconcile
           instead of one clobbering another. Staged from the Workshop's "Send to Modkit" or added
           by hand here. -->
      <section class="guilloche mt-6 rounded-xl border border-zinc-800 p-5">
        <div class="flex items-center justify-between">
          <div>
            <h3 class="plate-label">Shipments</h3>
            <p class="mt-1 text-xs text-zinc-500">
              Source mods, built and script-linked through Quartermaster when you build below.
              Sent from the Workshop, or add a local Shipment folder.
            </p>
          </div>
          <button class="btn-secondary shrink-0" @click="addShipment">Add Shipment…</button>
        </div>
        <ul v-if="shipments.length" class="mt-4 space-y-2">
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
        <p v-else class="mt-3 text-xs text-zinc-600">None added.</p>
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
        <div
          v-if="buildResult.path"
          class="engraved rounded-lg border border-zinc-800 bg-zinc-900/50 px-4 py-3 text-sm"
        >
          <p class="font-mono text-xs break-all text-zinc-200">{{ buildResult.path }}</p>
          <p class="mt-1 text-xs text-zinc-500">
            {{ buildResult.block_count }} block{{ buildResult.block_count === 1 ? "" : "s" }}
            · {{ fmtBytes(buildResult.byte_size) }}
          </p>
          <p class="mt-1 font-mono text-[11px] text-zinc-600">
            sha256 {{ buildResult.sha256.slice(0, 32) }}…
          </p>
        </div>
        <div
          v-else
          class="engraved rounded-lg border border-zinc-800 bg-zinc-900/50 px-4 py-3 text-sm text-zinc-400"
        >
          No patch WAD — nothing in this load order carries WAD assets. The files below are the
          whole build.
        </div>

        <!-- The game-folder half. Nothing else in the app shows what a Shipment drops into the
             game install, and an .asi is unrestricted native code in the game process. -->
        <template v-if="buildResult.placed_files?.length">
          <h4 class="plate-label mt-4 mb-2">Files for the game folder</h4>
          <ul class="space-y-1">
            <li
              v-for="f in buildResult.placed_files"
              :key="f.relative"
              class="engraved flex items-center gap-2 rounded-lg border border-zinc-800 bg-zinc-900/50 px-3 py-2 text-xs"
            >
              <span
                class="stamp"
                :class="f.relative.toLowerCase().endsWith('.asi') ? 'text-amber-300' : 'text-zinc-400'"
              >
                {{ f.relative.toLowerCase().endsWith(".asi") ? "plugin" : "file" }}
              </span>
              <span class="min-w-0 flex-1 truncate font-mono text-zinc-300">{{ f.relative }}</span>
              <span class="truncate text-zinc-600">{{ f.shipment }}</span>
            </li>
          </ul>
          <p
            v-if="buildResult.placed_files.some((f) => f.relative.toLowerCase().endsWith('.asi'))"
            class="mt-2 rounded border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-300"
          >
            A plugin (<code>.asi</code>) is native code that runs inside the game with no
            restrictions. Install it only from a Shipment you trust.
          </p>
        </template>

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
      <section
        v-if="validation"
        id="validation"
        ref="validationSection"
        class="mt-6 scroll-mt-6 rounded-lg p-1 transition-all duration-500"
        :class="highlightValidation ? 'ring-2 ring-brass-500/70 bg-brass-500/5' : ''"
      >
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
      <section
        v-if="buildResult"
        class="guilloche mt-6 rounded-xl border p-5"
        :class="validation && !validation.ok ? 'border-red-500/40' : 'border-zinc-800'"
      >
        <h3 class="plate-label">Install</h3>
        <p class="mt-1 text-xs text-zinc-500">
          Copies the WAD into
          <code>{{ gameInfo?.data_dir ?? "the game's data folder" }}</code
          >. Your current <code>vz-patch.wad</code> is backed up first, so you can undo
          this. <strong class="text-zinc-400">Close the game before installing.</strong>
        </p>
        <!-- The build failed validation: don't hide that behind an ordinary Install button. -->
        <p
          v-if="validation && !validation.ok"
          class="mt-2 rounded border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-300"
        >
          This patch <strong>failed validation</strong> (exit {{ validation.exit_code ?? "?" }}).
          Installing it may crash the game or corrupt a save — prefer fixing the load order above.
        </p>
        <button
          class="btn-plate mt-3 w-full justify-center"
          :class="
            validation && !validation.ok
              ? '!border-red-500 !bg-red-600 hover:!bg-red-500'
              : ''
          "
          :disabled="busy || !gameInfo?.data_dir"
          @click="deploy"
        >
          {{ validation && !validation.ok ? "Install anyway" : "Install into the game" }}
        </button>
        <p
          v-if="deployed"
          class="mt-3 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-300"
        >
          Installed to <span class="font-mono">{{ deployed }}</span
          >. Launch the game, then check Diagnostics if anything looks wrong.
        </p>

        <!-- What went into the game folder, and what came back out to make room. -->
        <div v-if="placement" class="mt-3 space-y-2 text-xs">
          <p
            v-if="placement.placed.length"
            class="rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-emerald-300"
          >
            Placed
            {{ placement.placed.length }} file{{ placement.placed.length === 1 ? "" : "s" }}:
            <span class="font-mono">{{ placement.placed.map((f) => f.relative).join(", ") }}</span>
          </p>
          <p
            v-if="placement.removed.length"
            class="rounded-lg border border-zinc-700 bg-zinc-900/60 px-3 py-2 text-zinc-400"
          >
            Removed from a previous install (recoverable from the trash):
            <span class="font-mono">{{ placement.removed.join(", ") }}</span>
          </p>
          <p
            v-if="placement.backed_up.length"
            class="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-amber-300"
          >
            Files that were already there were renamed to <code>.bak</code>:
            <span class="font-mono">{{ placement.backed_up.join(", ") }}</span>
          </p>
          <p
            v-if="placement.skipped.length"
            class="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-amber-300"
          >
            Left alone because they no longer match what modkit installed — remove them by hand if
            you meant to:
            <span class="font-mono">{{ placement.skipped.join(", ") }}</span>
          </p>
        </div>
      </section>

      <!-- Undo. Every hazard a bad WAD can cause is recoverable from here. -->
      <section class="guilloche mt-6 rounded-xl border border-zinc-800 p-5">
        <h3 class="plate-label">Undo</h3>
        <p class="mt-1 text-xs text-zinc-500">
          Restore a previous patch, or remove it entirely to go back to the unmodded game. Removing
          the patch also takes out any plugins and companion files a Shipment installed.
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
