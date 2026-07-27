<script setup lang="ts">
import { ref, computed, onMounted, watch } from "vue";
import { storeToRefs } from "pinia";
import { useProjectStore } from "../stores/project";
import type { WardrobeModel } from "../types";

const store = useProjectStore();
const { gameInfo, wardrobeModels, wardrobe, busy, error } = storeToRefs(store);

const HEROES = [
  { key: "mattias", label: "Mattias" },
  { key: "chris", label: "Chris" },
  { key: "jennifer", label: "Jennifer" },
] as const;

const hero = ref<string>("mattias");
const search = ref("");

onMounted(() => {
  if (gameInfo.value) void store.loadWardrobeModels().catch(() => {});
});
watch(gameInfo, (g) => {
  if (g) void store.loadWardrobeModels().catch(() => {});
});

/** Show only skins built like the currently-selected hero. */
const matchHero = ref(false);
/**
 * Hide skins already in the base wardrobe. On by default: adding one does nothing (it's
 * deduped out), so what you actually want are the skins the game doesn't already give you.
 */
const hideExisting = ref(true);

const filtered = computed<WardrobeModel[]>(() => {
  const q = search.value.trim().toLowerCase();
  return wardrobeModels.value
    .filter((m) => !hideExisting.value || !m.in_base_wardrobe)
    .filter((m) => !matchHero.value || m.closest_hero === hero.value)
    .filter(
      (m) =>
        !q ||
        m.label.toLowerCase().includes(q) ||
        m.model.toLowerCase().includes(q),
    );
});

/** Outfits already queued for the currently-selected character. */
const forHero = computed(() => wardrobe.value.filter((o) => o.hero === hero.value));

function isAdded(model: string): boolean {
  return wardrobe.value.some((o) => o.hero === hero.value && o.model === model);
}

function add(m: WardrobeModel) {
  if (isAdded(m.model)) return;
  store.addWardrobeOutfit({ hero: hero.value, model: m.model, label: m.label });
}
</script>

<template>
  <div class="mx-auto max-w-3xl px-8 py-6">
    <header>
      <h2 class="plate-title text-xl">Wardrobe</h2>
      <hr class="plate-rule my-3" />
      <p class="text-sm text-zinc-500">
        Wear any character the game already has. Pick a hero, pick a look — it shows up in
        the wardrobe in the PMC.
      </p>
    </header>

    <div
      v-if="!gameInfo"
      class="empty-plate mt-10"
    >
      Choose your game folder first.
    </div>

    <template v-else>
      <!-- These skins already exist in the game files, so nothing is injected and nothing
           can break. Worth telling the user, because it's why this is safe. -->
      <p
        class="mt-4 rounded-lg border border-zinc-800 bg-zinc-900/50 px-4 py-3 text-xs text-zinc-400"
      >
        These outfits use models already shipped in your copy of the game — no new files are
        added, so there's nothing to go wrong.
        <strong class="text-zinc-300"
          ><span class="serial text-emerald-400">{{ wardrobeModels.length }}</span> skins
          found.</strong
        >
        They're not a hand-written list: modkit checks which models are built on the
        <em>same skeleton</em> as the three player characters, which is what lets them play
        the same animations.
      </p>

      <!-- Hero. The game's own shell renders these three as engraved portraits
           in oval cartouches — one .bik each — so the picker borrows that
           framing. The portraits themselves stay in the user's install: the
           game files are copyrighted, so nothing is shipped with modkit. -->
      <section class="mt-6">
        <h3 class="plate-label">Character</h3>
        <div class="mt-3 flex gap-5">
          <button
            v-for="h in HEROES"
            :key="h.key"
            class="group flex flex-col items-center gap-2"
            :aria-pressed="hero === h.key"
            @click="hero = h.key"
          >
            <span
              class="cartouche guilloche flex h-[104px] w-[84px] items-center justify-center bg-zinc-900 transition"
              :class="
                hero === h.key
                  ? 'ring-1 ring-emerald-400/70'
                  : 'opacity-55 group-hover:opacity-90'
              "
            >
              <span
                class="text-2xl font-bold italic"
                :class="hero === h.key ? 'text-emerald-300' : 'text-zinc-500'"
              >
                {{ h.label[0] }}
              </span>
            </span>
            <span
              class="text-[10px] font-semibold uppercase tracking-[0.18em]"
              :class="hero === h.key ? 'text-brass-400' : 'text-zinc-500'"
            >
              {{ h.label }}
            </span>
          </button>
        </div>
      </section>

      <!-- Queued outfits for this hero -->
      <section v-if="forHero.length" class="mt-6">
        <h3 class="plate-label">
          Added for {{ HEROES.find((h) => h.key === hero)?.label }}
        </h3>
        <ul class="mt-2 space-y-2">
          <li
            v-for="o in forHero"
            :key="o.model"
            class="flex items-center gap-3 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2"
          >
            <div class="min-w-0 flex-1">
              <p class="truncate text-sm text-emerald-200">{{ o.label }}</p>
              <p class="truncate font-mono text-xs text-emerald-400/60">{{ o.model }}</p>
            </div>
            <button
              class="btn-outline"
              @click="store.removeWardrobeOutfit(o.hero, o.model)"
            >
              Remove
            </button>
          </li>
        </ul>
      </section>

      <!-- Picker -->
      <section class="mt-6">
        <h3 class="plate-label">Available looks</h3>
        <input
          v-model="search"
          placeholder="Search…"
          class="field mt-2 w-full"
        />

        <div class="mt-2 flex flex-wrap gap-3 text-xs text-zinc-400">
          <label class="flex items-center gap-1.5">
            <input v-model="hideExisting" type="checkbox" class="accent-emerald-500" />
            Hide outfits you already have
          </label>
          <label class="flex items-center gap-1.5">
            <input v-model="matchHero" type="checkbox" class="accent-emerald-500" />
            Only skins built like
            {{ HEROES.find((h) => h.key === hero)?.label }}
          </label>
          <span class="ml-auto"
            ><span class="serial text-emerald-400">{{ filtered.length }}</span>
            <span class="text-zinc-600"> shown</span></span
          >
        </div>

        <ul class="mt-3 space-y-2">
          <li
            v-for="m in filtered"
            :key="m.model"
            class="flex items-center gap-3 rounded-lg border border-zinc-800 bg-zinc-900/50 px-3 py-2"
          >
            <div class="min-w-0 flex-1">
              <p class="truncate text-sm text-zinc-200">
                {{ m.label }}
                <span
                  v-if="m.in_base_wardrobe"
                  class="stamp ml-1 text-zinc-600"
                  title="Already in your wardrobe — adding it changes nothing."
                >
                  already available
                </span>
              </p>
              <p class="truncate font-mono text-xs text-zinc-600">{{ m.model }}</p>
              <p class="mt-0.5 flex items-center gap-2 text-[11px]">
                <!-- The rig match is the whole basis for offering this skin at all, so show it. -->
                <span
                  :class="m.rig_match >= 0.999 ? 'text-emerald-400' : 'text-amber-300/80'"
                  :title="
                    m.rig_match >= 0.999
                      ? 'Has the complete player skeleton — animates exactly like a hero.'
                      : 'Missing a few of the player skeleton\'s bones. It will still work; those bones just won\'t animate.'
                  "
                >
                  {{ Math.round(m.rig_match * 100) }}% skeleton
                </span>
                <span class="text-zinc-600">built like {{ m.closest_hero }}</span>
                <span class="serial text-zinc-700">{{ m.triangles.toLocaleString() }} tris</span>
              </p>
            </div>
            <button
              v-if="isAdded(m.model)"
              class="stamp shrink-0 cursor-default text-zinc-600"
              disabled
            >
              Added
            </button>
            <button v-else class="btn-plate shrink-0" @click="add(m)">Add</button>
          </li>
        </ul>

        <p v-if="!filtered.length" class="mt-4 text-sm text-zinc-600">
          Nothing matches “{{ search }}”.
        </p>
      </section>

      <div
        v-if="error"
        class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
      >
        {{ error }}
      </div>

      <p v-if="wardrobe.length" class="mt-6 text-sm text-zinc-400">
        {{ wardrobe.length }} outfit{{ wardrobe.length === 1 ? "" : "s" }} queued. Go to
        <RouterLink to="/export" class="text-emerald-400 underline">Build &amp; Deploy</RouterLink>
        to put them in your game.
      </p>
      <p v-if="busy" class="mt-2 text-xs text-zinc-600">Working…</p>
    </template>
  </div>
</template>
