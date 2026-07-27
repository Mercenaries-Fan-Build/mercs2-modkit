<script setup lang="ts">
import { ref, watch, computed } from "vue";
import { storeToRefs } from "pinia";
import { open, save } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "../stores/project";
import Spinner from "../components/Spinner.vue";
import ModelViewer from "../components/ModelViewer.vue";
import TextureCanvas from "../components/TextureCanvas.vue";
import type {
  ModelGeometry,
  ModelRef,
  ModelVariant,
  TextureDetails,
  TextureExport,
  TexturePart,
} from "../types";

/** Texture name, from the route (`/texture/:name`). */
const props = defineProps<{ name: string }>();

const store = useProjectStore();
const { gameInfo, textures, busy } = storeToRefs(store);

const details = ref<TextureDetails | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
/**
 * Which model is currently shown in 3D — as a *reference* (its name, or `0x…`).
 *
 * Models are addressed by hash in the WAD, so an unnamed one renders just as well as a named
 * one. This matters: an atlas texture can be used by dozens of models, none of which we can
 * name, and refusing to show any of them made the whole page useless in exactly the case
 * where you most need it.
 */
const viewing = ref<string | null>(null);
/** State/LOD variants of that model, and whether each one paints the texture. */
const variants = ref<ModelVariant[]>([]);
/** The state bit currently shown. `null` = let the backend pick one that shows the texture. */
const tier = ref<number | null>(null);
/** Geometry of the model being viewed — shared with the UV overlay on the texture canvas. */
const geometry = ref<ModelGeometry | null>(null);
/** The part (draw group) the user has isolated, if any. */
const selectedPart = ref<number | null>(null);

/** Every part across every model that paints this texture. */
const allParts = ref<TexturePart[]>([]);

/**
 * A part id to select once its model's geometry has loaded.
 *
 * Part ids are per-model, so selecting one before the right geometry is in would apply it to
 * the previous model's part list. Changing `viewing` already triggers a reload, so we park the
 * id here and `loadGeometry` claims it — no second fetch, no race.
 */
const pendingPart = ref<number | null>(null);

/**
 * Jump straight to a part: switch to its model AND its tier, then isolate it.
 *
 * The tier is not optional. A part id indexes into the built group list, and that list depends
 * on which state bit was built — so the same id means a different part at a different tier.
 * `texture_parts` reports the tier it derived the id from; we pin it.
 */
function openPart(p: TexturePart) {
  if (viewing.value === p.model && tier.value === p.tier) {
    selectedPart.value = p.part;
    return;
  }
  pendingPart.value = p.part;
  tier.value = p.tier;
  viewing.value = p.model;
}

const showingVariants = computed(() => variants.value.filter((v) => v.shows_texture));

/**
 * The model's parts, with the ones using this texture first — that's what the user came for,
 * and a big model can have dozens of parts.
 */
const parts = computed(() =>
  [...(geometry.value?.groups ?? [])].sort(
    (a, b) =>
      Number(b.uses_texture) - Number(a.uses_texture) || b.triangles - a.triangles,
  ),
);

/**
 * Load the model's variants and select one that actually paints the texture.
 *
 * This is the point of the feature: instead of rendering a default state, shrugging "not
 * visible" and leaving the user stuck, we find the state where it IS visible and open there.
 */
async function loadVariants(model: string, texture: string) {
  variants.value = [];
  // Don't clobber a tier that "Everywhere it's painted" deliberately pinned — the part id it
  // queued is only valid for that tier.
  const pinned = pendingPart.value !== null;
  if (!pinned) tier.value = null;
  try {
    variants.value = await store.modelVariants(model, texture);
    if (!pinned) {
      tier.value = variants.value.find((v) => v.shows_texture)?.tier ?? null;
    }
  } catch {
    /* the viewer still works; it just won't offer state toggles */
  }
}

// Switching model re-derives its states.
watch(viewing, (m) => {
  if (m && details.value) void loadVariants(m, details.value.name);
});

/** Every model that paints this texture — named or not, all of them are viewable. */
const usedBy = computed<ModelRef[]>(() => details.value?.used_by ?? []);

/** Label for a model chip: its name if we know it, else its hash. */
function label(m: ModelRef): string {
  return m.name ?? hex(m.hash);
}

/**
 * Fetch the geometry once, here, so BOTH the 3D view and the texture's UV overlay use the
 * same data. The overlay is what makes an atlas legible: it draws the model's UV triangles
 * onto the image, showing exactly which patch of the sheet belongs to it.
 */
async function loadGeometry() {
  geometry.value = null;
  selectedPart.value = null; // part ids are per-model/per-state; never carry one across
  if (!viewing.value || !details.value) return;
  try {
    geometry.value = await store.modelGeometry(
      viewing.value,
      details.value.name,
      tier.value,
    );
    // Claim a selection queued by "Everywhere it's painted", now that the ids line up.
    if (pendingPart.value !== null) {
      selectedPart.value = pendingPart.value;
      pendingPart.value = null;
    }
  } catch {
    /* the 3D view surfaces its own error */
  }
}
watch([viewing, tier], loadGeometry);

async function exportPng() {
  if (!details.value) return;
  const dest = await save({
    title: "Save texture as PNG",
    defaultPath: `${details.value.name}.png`,
    filters: [{ name: "PNG", extensions: ["png"] }],
  });
  if (typeof dest !== "string") return;
  exported.value = await store.exportTexture(details.value.name, dest).catch((e) => {
    error.value = String(e);
    return null;
  });
}
const exported = ref<TextureExport | null>(null);

async function load(name: string) {
  if (!gameInfo.value) return;
  loading.value = true;
  error.value = null;
  details.value = null;
  viewing.value = null;
  geometry.value = null;
  exported.value = null;
  allParts.value = [];
  pendingPart.value = null;
  try {
    details.value = await store.textureDetails(name);
    // Any model that paints it, named or not — they're all loadable by hash.
    viewing.value = usedBy.value[0]?.reference ?? null;
    // The cross-model list; the usage index is already warm by now, so this is cheap.
    store
      .textureParts(name)
      .then((p) => (allParts.value = p))
      .catch(() => {});
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

// Re-fetch when the route param changes — following a "used alongside" link is a real
// navigation, so Back/Forward work and the URL is shareable.
watch(() => props.name, load, { immediate: true });
watch(gameInfo, () => load(props.name));

const queued = computed(() =>
  textures.value.find((t) => t.name === props.name),
);

async function replace() {
  const f = await open({
    title: `Choose an image to replace ${props.name}`,
    filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg"] }],
  });
  if (typeof f !== "string") return;
  store.addTextureSwap({ name: props.name, image_path: f });
}

function fmtKB(bytes: number): string {
  return bytes > 1024 * 1024
    ? `${(bytes / (1024 * 1024)).toFixed(1)} MB`
    : `${Math.round(bytes / 1024)} KB`;
}

function hex(n: number): string {
  return `0x${n.toString(16).toUpperCase().padStart(8, "0")}`;
}

function fileName(p: string): string {
  return p.split(/[\\/]/).pop() ?? p;
}
</script>

<template>
  <div class="mx-auto max-w-3xl px-8 py-6">
    <!-- Breadcrumb. `router.back()` isn't used: you can land here from a link or a deep
         link, and "Textures" is always the right place to go up to. -->
    <nav class="mb-5 flex items-center gap-2 text-sm">
      <RouterLink to="/textures" class="text-zinc-500 hover:text-zinc-300">
        ← Textures
      </RouterLink>
      <span class="text-zinc-700">/</span>
      <span class="truncate font-mono text-zinc-300">{{ name }}</span>
    </nav>

    <div
      v-if="!gameInfo"
      class="empty-plate"
    >
      Choose your game folder first.
    </div>

    <!-- The first lookup on an install builds the model -> texture index. -->
    <div v-else-if="loading" class="py-20 text-center">
      <Spinner class="mx-auto h-6 w-6" />
      <p class="mt-3 text-sm text-zinc-400">Working out where this texture is used…</p>
      <p class="mt-1 text-xs text-zinc-600">
        The first time, this reads every model in your game to see what uses what. It takes a
        few seconds, then it's remembered.
      </p>
    </div>

    <div
      v-else-if="error"
      class="rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
    >
      {{ error }}
    </div>

    <template v-else-if="details">
      <!-- Header -->
      <header class="flex flex-col items-start gap-6 md:flex-row">
        <!-- The texture itself: zoom into it, and see the model's UV layout drawn on top. -->
        <div class="w-full shrink-0 md:w-[340px]">
          <TextureCanvas
            v-if="details.preview"
            :src="details.preview.data_url"
            :width="details.width"
            :height="details.height"
            :geometry="geometry"
            :selected="selectedPart"
          />
          <button
            class="btn-outline mt-2 w-full justify-center"
            @click="exportPng"
          >
            Save as PNG…
          </button>
          <p
            v-if="exported"
            class="mt-1 text-[11px]"
            :class="exported.is_full_resolution ? 'text-emerald-400' : 'text-amber-300/80'"
          >
            Saved {{ exported.width }}×{{ exported.height }}.
            <template v-if="!exported.is_full_resolution">
              That's the largest version stored with this texture — the game streams the rest,
              so its full {{ exported.full_width }}×{{ exported.full_height }} detail isn't in
              the file.
            </template>
          </p>
        </div>

        <div class="min-w-0 flex-1">
          <h2 class="break-all font-mono text-lg font-semibold text-zinc-100">
            {{ details.name }}
          </h2>
          <p class="mt-1 text-sm text-zinc-500">
            {{ details.width }}×{{ details.height }} · {{ details.format }} ·
            {{ details.mip_count }} mips · {{ fmtKB(details.chain_bytes) }}
          </p>
          <p class="mt-1 font-mono text-xs text-zinc-600">{{ hex(details.asset_hash) }}</p>

          <div class="mt-3 flex flex-wrap gap-1.5">
            <span class="rounded-full bg-zinc-800 px-2.5 py-0.5 text-xs text-zinc-400">
              {{ details.category }}
            </span>
            <span class="rounded-full bg-zinc-800 px-2.5 py-0.5 text-xs text-zinc-400">
              {{ details.kind }}
            </span>
            <span
              class="stamp"
              :class="
                details.fully_resident
                  ? 'bg-zinc-800 text-zinc-400'
                  : 'text-sky-300'
              "
            >
              {{ details.fully_resident ? "stored in full" : "streamed" }}
            </span>
          </div>

          <p
            v-if="
              details.preview && details.preview.preview_width < details.width
            "
            class="mt-3 text-xs text-zinc-600"
          >
            The preview above is only
            {{ details.preview.preview_width }}×{{ details.preview.preview_height }} — the
            game streams this texture's detail from elsewhere, so that's the largest version
            stored with it. Your replacement is still used at the full
            {{ details.width }}×{{ details.height }}.
          </p>
        </div>
      </header>

      <!-- Replace -->
      <section class="guilloche mt-6 rounded-xl border border-zinc-800 p-5">
        <div
          v-if="queued"
          class="mb-3 flex items-center gap-2 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-300"
        >
          <span class="min-w-0 flex-1 truncate">
            Replacing with <strong>{{ fileName(queued.image_path) }}</strong>
          </span>
          <button
            class="rounded border border-emerald-500/40 px-2 py-0.5 hover:bg-emerald-500/20"
            @click="store.removeTextureSwap(details.name)"
          >
            Undo
          </button>
        </div>

        <p class="text-xs text-zinc-500">
          Your image is resized to {{ details.width }}×{{ details.height }} to match the game.
          Higher-resolution replacements aren't supported yet.
        </p>
        <button
          class="btn-plate mt-3 w-full justify-center"
          :disabled="busy"
          @click="replace"
        >
          {{ queued ? "Choose a different image…" : "Choose an image…" }}
        </button>
        <p v-if="textures.length" class="mt-3 text-center text-xs text-zinc-500">
          <RouterLink to="/export" class="text-emerald-400 underline">Build &amp; Deploy</RouterLink>
          to put your {{ textures.length }} replacement{{ textures.length === 1 ? "" : "s" }} in
          the game.
        </p>
      </section>

      <!-- Where it's used: the reason this page exists. -->
      <section class="mt-6">
        <h3 class="plate-label">Used by</h3>

        <p
          v-if="details.shared"
          class="mt-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-4 py-3 text-sm text-amber-300"
        >
          <strong>{{ details.used_by.length }} different models share this texture.</strong>
          Replacing it changes all of them, not just one.
        </p>

        <p
          v-if="!details.used_by.length && details.declared_only_by.length"
          class="mt-2 text-sm text-zinc-500"
        >
          No model paints this texture on a visible surface — so there's nothing to show in 3D.
          It's still referenced by the models below; see the note there.
        </p>
        <p
          v-else-if="!details.used_by.length"
          class="mt-2 text-sm text-zinc-600"
        >
          No model in your game uses this texture. It's probably used by the interface or an
          effect — or it's unused leftover art.
        </p>

        <template v-else>
          <!-- Pick which model to look at. Unnamed models are included: the WAD addresses
               models by hash, so they load and render exactly like named ones. -->
          <div v-if="usedBy.length > 1" class="mt-3 flex flex-wrap gap-2">
            <button
              v-for="m in usedBy"
              :key="m.hash"
              class="rounded-lg border px-3 py-1.5 font-mono text-xs"
              :class="
                viewing === m.reference
                  ? 'border-emerald-500 bg-emerald-500/15 text-emerald-300'
                  : m.name
                    ? 'border-zinc-700 text-zinc-300 hover:bg-zinc-800'
                    : 'border-zinc-800 text-zinc-500 hover:bg-zinc-800'
              "
              :title="m.name ? m.name : 'This model has no known name, but it still renders.'"
              @click="viewing = m.reference"
            >
              {{ label(m) }}
            </button>
          </div>

          <!-- See exactly WHERE the texture lands, not just which models list it. -->
          <ModelViewer
            v-if="viewing"
            :key="`${viewing}:${details.name}:${tier ?? 'auto'}`"
            class="mt-3"
            :model="viewing"
            :texture="details.name"
            :texture-url="details.preview?.data_url ?? null"
            :tier="tier"
            :selected="selectedPart"
          />

          <!--
            The conditions under which the texture is (or isn't) painted.

            A model's parts are gated by a state bit, and those bits are NOT a detail ladder —
            they're state masks, so a texture can be on one state and absent from another.
            Rather than saying "not visible" and stopping, show every state and which ones
            actually show it, and let the user flip between them.
          -->
          <div v-if="variants.length > 1" class="mt-3">
            <p class="text-xs text-zinc-500">
              This model has {{ variants.length }} versions (detail levels / damage states).
              <span v-if="showingVariants.length">
                The texture is painted on
                <strong class="text-emerald-400">{{ showingVariants.length }}</strong> of them.
              </span>
            </p>
            <div class="mt-2 flex flex-wrap gap-1.5">
              <button
                v-for="(v, i) in variants"
                :key="i"
                class="rounded-lg border px-2.5 py-1 text-xs"
                :class="[
                  tier === v.tier
                    ? 'border-emerald-500 bg-emerald-500/15 text-emerald-300'
                    : 'border-zinc-700 text-zinc-400 hover:bg-zinc-800',
                  !v.shows_texture && 'opacity-60',
                ]"
                :title="
                  v.shows_texture
                    ? `${v.highlighted} part(s) use this texture`
                    : 'This texture is not painted on this version'
                "
                @click="tier = v.tier"
              >
                <span v-if="v.shows_texture" class="text-emerald-400">●</span>
                <span v-else class="text-zinc-600">○</span>
                {{ v.tier === null ? "All parts" : `Version ${i + 1}` }}
                <span class="text-zinc-600">
                  {{ v.triangles.toLocaleString() }} tris
                </span>
              </button>
            </div>
          </div>

          <!--
            The model's PARTS. A model isn't one mesh — Chris is 25 separate draw groups, each
            with its own material (eyes, teeth, head, upper body, the pistol he's holding).
            Listing them turns "5 of 25 parts use this texture" into something you can act on:
            which part, how big, what else it wears, and where it sits on the sheet.
          -->
          <div v-if="parts.length" class="mt-4">
            <div class="flex items-center justify-between">
              <h4 class="plate-label">
                Parts of this model ({{ parts.length }})
              </h4>
              <button
                v-if="selectedPart !== null"
                class="btn-outline"
                @click="selectedPart = null"
              >
                Show all
              </button>
            </div>
            <p class="mt-1 text-xs text-zinc-600">
              Click a part to isolate it in the model and on the texture.
            </p>

            <ul class="mt-2 max-h-72 space-y-1 overflow-y-auto pr-1">
              <li v-for="p in parts" :key="p.id">
                <button
                  class="w-full rounded-lg border px-3 py-2 text-left"
                  :class="
                    selectedPart === p.id
                      ? 'border-sky-500 bg-sky-500/10'
                      : p.uses_texture
                        ? 'border-emerald-500/40 bg-emerald-500/5 hover:bg-emerald-500/10'
                        : 'border-zinc-800 bg-zinc-900/40 hover:bg-zinc-800/60'
                  "
                  @click="selectedPart = selectedPart === p.id ? null : p.id"
                >
                  <div class="flex items-center gap-2">
                    <span
                      class="h-2 w-2 shrink-0 rounded-full"
                      :class="p.uses_texture ? 'bg-emerald-400' : 'bg-zinc-700'"
                    />
                    <span class="text-xs text-zinc-300">Part {{ p.id + 1 }}</span>
                    <span class="text-xs text-zinc-600">
                      {{ p.triangles.toLocaleString() }} tris
                    </span>
                    <span
                      v-if="p.uses_texture"
                      class="ml-auto rounded-full bg-emerald-500/15 px-2 py-0.5 text-[11px] text-emerald-300"
                    >
                      uses this as {{ p.slot }}
                    </span>
                  </div>

                  <!-- Every map this part wears. Clicking one jumps to that texture. -->
                  <div class="mt-1.5 flex flex-wrap gap-1">
                    <RouterLink
                      v-for="s in p.textures"
                      :key="s.slot + s.hash"
                      :to="s.name ? `/texture/${encodeURIComponent(s.name)}` : ''"
                      class="rounded border px-1.5 py-0.5 font-mono text-[11px]"
                      :class="
                        s.is_current
                          ? 'border-emerald-500/50 text-emerald-300'
                          : s.name
                            ? 'border-zinc-700 text-zinc-500 hover:bg-zinc-800 hover:text-zinc-300'
                            : 'pointer-events-none border-zinc-800 text-zinc-700'
                      "
                      @click.stop
                    >
                      <span class="text-zinc-600">{{ s.slot }}</span>
                      {{ s.name ?? hex(s.hash) }}
                    </RouterLink>
                  </div>
                </button>
              </li>
            </ul>
          </div>

          <p v-if="usedBy.some((m) => !m.name)" class="mt-2 text-xs text-zinc-600">
            Some of these models have no known name, so they're listed by ID — they still open
            in the viewer.
          </p>
        </template>

        <!--
          The honest bucket. A model's material can name a texture that no part of it ever
          paints — because the geometry using it is the model's WRECK, or a separate
          sub-model (a tank's tracks are their own model), or a merged low-detail version.
          Saying so beats pretending we can show it.
        -->
        <div v-if="details.declared_only_by.length" class="mt-4">
          <h4 class="plate-label">
            Also referenced by (but not painted on)
          </h4>
          <p class="mt-1 text-xs text-zinc-600">
            These models list this texture in their materials, but none of their visible parts
            use it — it belongs to their wrecked version, or to a separate piece that's its own
            model.
          </p>
          <ul class="mt-2 flex flex-wrap gap-2">
            <li
              v-for="m in details.declared_only_by"
              :key="m.hash"
              class="rounded-lg border border-zinc-800 bg-zinc-900/40 px-3 py-1.5 font-mono text-xs text-zinc-500"
            >
              {{ m.name ?? hex(m.hash) }}
            </li>
          </ul>
        </div>
      </section>

      <!--
        Every part across every model — not just the one on screen. For a texture 34 models
        share, clicking through each of them in turn to find out what it's actually on is not
        a workable answer.
      -->
      <section v-if="allParts.length" class="mt-6">
        <h3 class="plate-label">
          Everywhere it's painted
          <span class="text-zinc-600">({{ allParts.length }} parts)</span>
        </h3>
        <p class="mt-1 text-xs text-zinc-500">
          Biggest first — that's where a repaint actually shows. Click one to open it.
        </p>

        <ul class="mt-2 max-h-64 space-y-1 overflow-y-auto pr-1">
          <li v-for="(p, i) in allParts" :key="i">
            <button
              class="flex w-full items-center gap-2 rounded-lg border px-3 py-1.5 text-left"
              :class="
                viewing === p.model
                  ? 'border-emerald-500/40 bg-emerald-500/5'
                  : 'border-zinc-800 bg-zinc-900/40 hover:bg-zinc-800/60'
              "
              @click="openPart(p)"
            >
              <span
                class="min-w-0 flex-1 truncate font-mono text-xs"
                :class="p.model_name ? 'text-zinc-300' : 'text-zinc-500'"
              >
                {{ p.model_name ?? hex(p.model_hash) }}
              </span>
              <span class="shrink-0 text-[11px] text-zinc-600">part {{ p.part + 1 }}</span>
              <span class="shrink-0 text-[11px] text-zinc-600">
                {{ p.triangles.toLocaleString() }} tris
              </span>
              <span
                class="stamp shrink-0 text-zinc-500"
              >
                {{ p.slot }}
              </span>
            </button>
          </li>
        </ul>
      </section>

      <!-- Other maps of the same surface. -->
      <section v-if="details.siblings.length" class="mt-6">
        <h3 class="plate-label">Other maps of this surface</h3>
        <p class="mt-1 text-xs text-zinc-500">
          The same surface's colour, bump and shine maps.
        </p>
        <div class="mt-3 flex flex-wrap gap-2">
          <RouterLink
            v-for="s in details.siblings"
            :key="s.name"
            :to="`/texture/${encodeURIComponent(s.name)}`"
            class="rounded-lg border border-zinc-700 px-3 py-1.5 font-mono text-xs text-zinc-300 hover:bg-zinc-800"
          >
            {{ s.name }}
            <span class="text-zinc-600">{{ s.kind }}</span>
          </RouterLink>
        </div>
      </section>

      <!-- The rest of the character / vehicle. -->
      <section v-if="details.seen_with.length" class="mt-6">
        <h3 class="plate-label">
          Used alongside
          <span class="text-zinc-600">({{ details.seen_with.length }})</span>
        </h3>
        <p class="mt-1 text-xs text-zinc-500">
          The other textures on the same models — the rest of this character or vehicle.
        </p>
        <div class="mt-3 flex flex-wrap gap-2">
          <RouterLink
            v-for="s in details.seen_with"
            :key="s.name"
            :to="`/texture/${encodeURIComponent(s.name)}`"
            class="rounded-lg border border-zinc-800 px-3 py-1.5 font-mono text-xs text-zinc-400 hover:bg-zinc-800"
          >
            {{ s.name }}
          </RouterLink>
        </div>
      </section>
    </template>
  </div>
</template>
