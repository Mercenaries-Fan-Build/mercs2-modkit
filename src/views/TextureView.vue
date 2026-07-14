<script setup lang="ts">
import { ref, computed, watch, onMounted } from "vue";
import { storeToRefs } from "pinia";
import { useProjectStore } from "../stores/project";
import Spinner from "../components/Spinner.vue";
import type { TextureEntry, TexturePreview } from "../types";

const store = useProjectStore();
const { gameInfo, textures, textureCatalog, error } = storeToRefs(store);

const search = ref("");
const kind = ref<"all" | "diffuse" | "normal" | "specular">("all");
const group = ref<string>("all");
const page = ref(0);
const PAGE = 48;

const loading = ref(false);
const previews = ref<Record<string, TexturePreview>>({});

/** Details live on their own page (`/texture/:name`) — deep-linkable, Back/Forward work. */
function detailsLink(name: string): string {
  return `/texture/${encodeURIComponent(name)}`;
}

onMounted(() => {
  if (gameInfo.value) void load();
});
watch(gameInfo, (g) => {
  if (g) void load();
});

async function load() {
  loading.value = true;
  try {
    await store.loadTextureCatalog();
  } finally {
    loading.value = false;
  }
}

/** The groups the game itself uses (`pmc_hum_chris_ub` -> `pmc`), biggest first. */
const groups = computed(() => {
  const counts = new Map<string, number>();
  for (const t of textureCatalog.value) {
    counts.set(t.category, (counts.get(t.category) ?? 0) + 1);
  }
  return [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 14);
});

/**
 * Score one token against a name. Names are underscore-separated words
 * (`pmc_hum_chris_ub`), and matching on a word boundary is far more likely to be what the
 * user meant than a match buried mid-word — a plain substring search for "eva" happily
 * returns `al_veh_truck_mtv_expandabl(e_va)n`, which is noise.
 *
 * 0 = no match; higher = better.
 */
function scoreToken(words: string[], name: string, tok: string): number {
  if (words.some((w) => w === tok)) return 4; // whole word:   "chris" in pmc_hum_chris_ub
  if (words.some((w) => w.startsWith(tok))) return 3; // word prefix:  "chr" -> chris
  if (words.some((w) => w.includes(tok))) return 2; // inside a word
  return name.includes(tok) ? 1 : 0; // spans words (weakest)
}

/**
 * Forgiving search: split the query on spaces and keep anything containing ALL the pieces,
 * in any order, anywhere in the name — so "boss head" finds `al_hum_boss_head` and you
 * never need the exact name. Results are then ranked so word-boundary hits come first.
 */
const matches = computed<TextureEntry[]>(() => {
  const toks = search.value.toLowerCase().split(/\s+/).filter(Boolean);

  const pool = textureCatalog.value.filter((t) => {
    if (kind.value !== "all" && t.kind !== kind.value) return false;
    if (group.value !== "all" && t.category !== group.value) return false;
    return true;
  });
  if (!toks.length) return pool;

  const scored: { t: TextureEntry; s: number }[] = [];
  for (const t of pool) {
    const name = t.name.toLowerCase();
    const words = name.split("_");
    let total = 0;
    let ok = true;
    for (const tok of toks) {
      const s = scoreToken(words, name, tok);
      if (s === 0) {
        ok = false;
        break;
      }
      total += s;
    }
    // Nudge shorter names up: `pmc_hum_chris_ub` beats `pmc_hum_chris_v3_acc_sm` for "chris".
    if (ok) scored.push({ t, s: total * 100 - name.length });
  }

  scored.sort((a, b) => b.s - a.s || a.t.name.localeCompare(b.t.name));
  return scored.map((x) => x.t);
});

const pageItems = computed(() => matches.value.slice(0, (page.value + 1) * PAGE));
const hasMore = computed(() => pageItems.value.length < matches.value.length);

// Reset paging whenever the filter changes, then fetch thumbnails for what's on screen.
watch([search, kind, group], () => {
  page.value = 0;
});
// `immediate` so the first page of thumbnails loads on open, not only after a filter change.
watch(
  pageItems,
  async (items) => {
    const missing = items.map((t) => t.name).filter((n) => !previews.value[n]);
    if (!missing.length) return;
    const got = await store.loadTexturePreviews(missing).catch(() => []);
    for (const p of got) previews.value[p.name] = p;
  },
  { immediate: true },
);

// Queued replacements are shown with a thumbnail too, and they may not be on the current
// page — fetch any we haven't decoded yet.
watch(
  textures,
  async (list) => {
    const missing = list.map((t) => t.name).filter((n) => !previews.value[n]);
    if (!missing.length) return;
    const got = await store.loadTexturePreviews(missing).catch(() => []);
    for (const p of got) previews.value[p.name] = p;
  },
  { immediate: true, deep: true },
);

function isQueued(name: string): boolean {
  return textures.value.some((t) => t.name === name);
}

function fileName(p: string): string {
  return p.split(/[\\/]/).pop() ?? p;
}
</script>

<template>
  <div class="mx-auto max-w-5xl px-8 py-6">
    <header>
      <h2 class="text-xl font-semibold">Textures</h2>
      <p class="text-sm text-zinc-500">
        Browse every texture in your game and swap in your own image.
      </p>
    </header>

    <div
      v-if="!gameInfo"
      class="mt-10 rounded-xl border border-dashed border-zinc-800 px-8 py-16 text-center text-zinc-500"
    >
      Choose your game folder first.
    </div>

    <template v-else>
      <!-- Queued replacements -->
      <section v-if="textures.length" class="mt-6">
        <h3 class="text-sm font-medium text-zinc-300">
          Your replacements ({{ textures.length }})
        </h3>
        <ul class="mt-2 space-y-2">
          <li
            v-for="t in textures"
            :key="t.name"
            class="flex items-center gap-3 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2"
          >
            <img
              v-if="previews[t.name]"
              :src="previews[t.name].data_url"
              class="h-10 w-10 rounded border border-emerald-500/30 object-cover"
              alt=""
            />
            <RouterLink :to="detailsLink(t.name)" class="min-w-0 flex-1">
              <p class="truncate font-mono text-sm text-emerald-200">{{ t.name }}</p>
              <p class="truncate text-xs text-emerald-400/60">→ {{ fileName(t.image_path) }}</p>
            </RouterLink>
            <button
              class="rounded-lg border border-zinc-700 px-3 py-1 text-xs text-zinc-300 hover:bg-zinc-800"
              @click="store.removeTextureSwap(t.name)"
            >
              Remove
            </button>
          </li>
        </ul>
        <p class="mt-3 text-sm text-zinc-400">
          Go to
          <RouterLink to="/export" class="text-emerald-400 underline">Build &amp; Deploy</RouterLink>
          to put {{ textures.length === 1 ? "it" : "them" }} in your game.
        </p>
      </section>

      <!-- Search + filters -->
      <section class="mt-6 rounded-xl border border-zinc-800 p-5">
        <input
          v-model="search"
          placeholder="Search — try “chris”, “tank”, or “boss head”…"
          class="w-full rounded-lg border border-zinc-700 bg-zinc-900 px-3 py-2 text-sm"
        />

        <div class="mt-3 flex flex-wrap gap-1.5">
          <button
            v-for="k in (['all', 'diffuse', 'normal', 'specular'] as const)"
            :key="k"
            class="rounded-full border px-3 py-1 text-xs capitalize"
            :class="
              kind === k
                ? 'border-emerald-500 bg-emerald-500/15 text-emerald-300'
                : 'border-zinc-700 text-zinc-400 hover:bg-zinc-800'
            "
            @click="kind = k"
          >
            {{ k === "all" ? "All types" : k }}
          </button>
        </div>

        <div class="mt-2 flex flex-wrap gap-1.5">
          <button
            class="rounded-full border px-3 py-1 text-xs"
            :class="
              group === 'all'
                ? 'border-emerald-500 bg-emerald-500/15 text-emerald-300'
                : 'border-zinc-700 text-zinc-400 hover:bg-zinc-800'
            "
            @click="group = 'all'"
          >
            All groups
          </button>
          <button
            v-for="[g, n] in groups"
            :key="g"
            class="rounded-full border px-3 py-1 text-xs"
            :class="
              group === g
                ? 'border-emerald-500 bg-emerald-500/15 text-emerald-300'
                : 'border-zinc-700 text-zinc-400 hover:bg-zinc-800'
            "
            @click="group = g"
          >
            {{ g }} <span class="text-zinc-600">{{ n }}</span>
          </button>
        </div>

        <p class="mt-3 text-xs text-zinc-500">
          <Spinner v-if="loading" class="mr-1 inline h-3 w-3" />
          <template v-if="loading">Reading your game’s textures…</template>
          <template v-else>
            {{ matches.length.toLocaleString() }} of
            {{ textureCatalog.length.toLocaleString() }} textures
          </template>
        </p>
      </section>

      <!-- Grid -->
      <section class="mt-6">
        <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 md:grid-cols-4">
          <RouterLink
            v-for="t in pageItems"
            :key="t.name"
            :to="detailsLink(t.name)"
            class="group block rounded-lg border p-2 text-left"
            :class="
              isQueued(t.name)
                ? 'border-emerald-500/50 bg-emerald-500/10'
                : 'border-zinc-800 bg-zinc-900/50 hover:border-zinc-600'
            "
          >
            <div
              class="flex aspect-square items-center justify-center overflow-hidden rounded bg-black/40"
            >
              <img
                v-if="previews[t.name]"
                :src="previews[t.name].data_url"
                class="h-full w-full object-cover"
                :alt="t.name"
              />
              <span v-else class="text-xs text-zinc-700">…</span>
            </div>
            <p class="mt-2 truncate font-mono text-xs text-zinc-300">{{ t.name }}</p>
            <p class="truncate text-[11px] text-zinc-600">
              <span v-if="previews[t.name]">
                {{ previews[t.name].width }}×{{ previews[t.name].height }}
              </span>
              <span v-else>{{ t.kind }}</span>
            </p>
          </RouterLink>
        </div>

        <p v-if="!matches.length && !loading" class="mt-8 text-center text-sm text-zinc-600">
          Nothing matches “{{ search }}”.
        </p>

        <button
          v-if="hasMore"
          class="mt-5 w-full rounded-lg border border-zinc-700 px-4 py-2 text-sm text-zinc-300 hover:bg-zinc-800"
          @click="page++"
        >
          Show more ({{ (matches.length - pageItems.length).toLocaleString() }} left)
        </button>
      </section>

      <div
        v-if="error"
        class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
      >
        {{ error }}
      </div>
    </template>
  </div>
</template>
