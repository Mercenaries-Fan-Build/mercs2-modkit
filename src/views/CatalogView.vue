<script setup lang="ts">
import { onMounted, ref } from "vue";
import { storeToRefs } from "pinia";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useProjectStore } from "../stores/project";
import type { CatalogMod, RegistryMod } from "../types";
import ProgressBar from "../components/ProgressBar.vue";

const store = useProjectStore();
const {
  catalog,
  catalogSource,
  busy,
  error,
  gameInfo,
  customSources,
  registryMods,
  registryStale,
  registryWarning,
} = storeToRefs(store);

const working = ref<string | null>(null); // repository#slug currently acting on
const lastAction = ref<string | null>(null);

const showSources = ref(false);
const newSourceUrl = ref("");
const sourceError = ref<string | null>(null);
const sourceWorking = ref(false);

function isGithubUrl(url: string): boolean {
  return /^https?:\/\/(www\.)?github\.com\/.+\/.+/.test(url.trim());
}

function sourceBranch(src: { branch?: string }): string | null {
  return src.branch ?? null;
}

async function addSource() {
  const url = newSourceUrl.value.trim();
  if (!isGithubUrl(url)) {
    sourceError.value = "Please enter a valid GitHub repository URL (https://github.com/owner/repo)";
    return;
  }
  sourceError.value = null;
  sourceWorking.value = true;
  try {
    await store.addCustomSource(url);
    newSourceUrl.value = "";
    await store.fetchCatalog();
    lastAction.value = "Source added and catalog refreshed";
  } catch (e) {
    sourceError.value = String(e);
  } finally {
    sourceWorking.value = false;
  }
}

async function removeSource(repository: string) {
  sourceWorking.value = true;
  try {
    await store.removeCustomSource(repository);
    await store.fetchCatalog();
  } finally {
    sourceWorking.value = false;
  }
}

onMounted(() => {
  if (store.catalog.length === 0) store.fetchCatalog();
  if (store.registryMods.length === 0) store.fetchRegistry();
});

/**
 * Install a Shipment from mercs.ink.
 *
 * Deliberately a *different verb* from the repository catalogue's "Download". A Shipment is
 * built into the patch WAD at assemble time rather than copied into the game folder, so it has
 * no download → enable → deploy ladder to climb; saying "Download" would promise a lifecycle
 * that does not exist here.
 */
async function install(item: RegistryMod) {
  working.value = `mercs.ink#${item.slug}`;
  lastAction.value = null;
  try {
    const res = await store.installFromRegistry(item);
    const files = `${res.staged_files} file${res.staged_files === 1 ? "" : "s"}`;
    lastAction.value = `Installed ${res.title ?? res.slug} ${res.release_version} (${files}) — it is staged for your next build`;
  } catch {
    /* surfaced via store.error */
  } finally {
    working.value = null;
  }
}

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
      <h2 class="plate-title text-xl">Browse Mods</h2>
      <p class="text-sm text-zinc-500">
        Two sources, listed separately. They are not two views of one catalogue:
        a mercs.ink mod is identified by the registry, a repository mod by the
        repo it lives in, and those identities cannot be compared — the same
        name in both lists may be the same mod or may not.
      </p>
    </header>

    <div class="mt-4 flex gap-2">
      <button
        class="btn-outline"
        :disabled="busy"
        @click="store.fetchCatalog(); store.fetchRegistry()"
      >
        Refresh
      </button>
      <button class="btn-outline" @click="showSources = !showSources">
        Repository sources
        <span class="ml-1 rounded bg-zinc-800 px-1 text-zinc-500">{{ customSources.length }}</span>
      </button>
    </div>

    <ProgressBar
      v-if="busy && !working"
      indeterminate
      label="Loading mods…"
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

    <!-- Custom sources: the repository catalogue's own list; mercs.ink is one registry. -->
    <div
      v-if="showSources"
      class="mt-4 rounded-xl border border-zinc-700 bg-zinc-900/70 p-4"
    >
      <h3 class="plate-label">Custom Mod Sources</h3>
      <p class="mt-0.5 text-xs text-zinc-500">
        Any GitHub repository that provides a <code class="text-zinc-400">repository.json</code> index
        can be added as a source. Its mods appear in the catalog after a refresh.
        This list is the repository catalogue's only — mercs.ink is one registry
        and has nothing to add sources to.
      </p>

      <!-- Existing custom sources -->
      <ul v-if="customSources.length" class="mt-3 space-y-2">
        <li
          v-for="src in customSources"
          :key="src.repository"
          class="engraved flex items-center justify-between gap-3 rounded-lg border border-zinc-800 bg-zinc-950 px-3 py-2"
        >
          <div class="min-w-0">
            <p class="truncate text-xs font-medium text-zinc-300">{{ src.name }}</p>
            <button
              class="truncate text-xs text-sky-400 hover:underline"
              @click="openUrl(src.repository)"
            >
              {{ src.repository }}
            </button>
            <span
              v-if="sourceBranch(src)"
              class="ml-1 rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-400"
            >{{ sourceBranch(src) }}</span>
          </div>
          <button
            class="shrink-0 rounded px-2 py-1 text-xs text-zinc-500 hover:bg-red-900/40 hover:text-red-300 disabled:opacity-50"
            :disabled="sourceWorking"
            @click="removeSource(src.repository)"
          >
            Remove
          </button>
        </li>
      </ul>
      <p v-else class="mt-3 text-xs text-zinc-600">No custom sources added yet.</p>

      <!-- Add source form -->
      <div class="mt-4 flex gap-2">
        <input
          v-model="newSourceUrl"
          type="url"
          placeholder="https://github.com/owner/repo  (or /tree/branch-name)"
          class="field min-w-0 flex-1 py-1.5"
          @keydown.enter="addSource"
        />
        <button
          class="shrink-0 rounded-md bg-sky-700 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-600 disabled:opacity-50"
          :disabled="sourceWorking || !newSourceUrl.trim()"
          @click="addSource"
        >
          Add
        </button>
      </div>
      <p v-if="sourceError" class="mt-2 text-xs text-red-400">{{ sourceError }}</p>
    </div>

    <!-- ─────────────────── mercs.ink — the community registry ─────────────────── -->
    <section class="mt-8">
      <div class="flex items-end justify-between gap-4">
        <div>
          <h3 class="plate-label">mercs.ink</h3>
          <p class="mt-0.5 text-xs text-zinc-500">
            Quartermaster Shipments. Installed as source and built into your
            patch WAD, so several script-touching mods compose instead of one
            clobbering another.
          </p>
        </div>
        <span class="shrink-0 text-xs text-zinc-600">{{ registryMods.length }} mod(s)</span>
      </div>

      <!--
        Stale is a disclosure, not a failure: the registry could not be reached and this is the
        last copy modkit downloaded. Installing from it stays allowed on purpose — a cached
        catalogue is still a usable one, and the artifacts come from GitHub either way.
      -->
      <div
        v-if="registryStale && registryWarning"
        class="mt-3 rounded-lg border border-amber-500/30 bg-amber-500/10 px-4 py-3 text-sm text-amber-300"
      >
        {{ registryWarning }}
      </div>

      <ul v-if="registryMods.length" class="mt-4 space-y-3">
        <li
          v-for="item in registryMods"
          :key="item.id ?? item.slug"
          class="guilloche rounded-xl border border-zinc-800 bg-zinc-900/50 p-4"
        >
          <div class="flex items-start justify-between gap-4">
            <div class="min-w-0">
              <div class="flex flex-wrap items-center gap-2">
                <p class="font-medium text-zinc-100">{{ item.title ?? item.slug }}</p>
                <!-- Where this row came from. Never omitted: the two lists' ids are not
                     comparable, so a row without its source is a row you can misread. -->
                <span class="stamp text-sky-300" title="From the mercs.ink registry">mercs.ink</span>
                <span
                  v-if="item.latest_version"
                  class="rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-400"
                  >v{{ item.latest_version }}</span
                >
                <!-- qm's Target: which build of the game this Shipment declares it fits.
                     Not the same thing as which build you are running. -->
                <span
                  v-if="item.target"
                  class="rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] uppercase text-zinc-500"
                  :title="`This Shipment declares compatibility with: ${item.target}`"
                  >{{ item.target }}</span
                >
                <span
                  v-if="store.registryUpdate(item)"
                  class="stamp text-amber-300"
                  :title="`A newer release (v${store.registryUpdate(item)}) is available — your staged copy is older`"
                  >update available</span
                >
                <span
                  v-else-if="store.registryInstalled(item)"
                  class="stamp text-emerald-300"
                  title="Staged in the load order — it goes into your next build"
                  >installed</span
                >
              </div>
              <p v-if="item.description" class="mt-0.5 text-sm text-zinc-400">
                {{ item.description }}
              </p>
              <p class="mt-1 text-xs text-zinc-600">
                <span v-if="item.authors.length">{{ item.authors.join(", ") }} · </span>
                <span>{{ item.slug }}</span>
                <span v-if="item.license"> · {{ item.license }}</span>
              </p>
              <button
                v-if="item.repository"
                class="mt-1 truncate text-xs text-sky-400 hover:underline"
                @click="openUrl(item.repository)"
              >
                {{ item.repository }}
              </button>
            </div>

            <div class="flex shrink-0 items-center gap-2">
              <button
                class="btn-plate"
                :disabled="busy"
                :title="
                  store.registryInstalled(item)
                    ? 'Re-stage this Shipment at the latest release'
                    : 'Stage this Shipment for your next patch WAD build'
                "
                @click="install(item)"
              >
                {{ store.registryUpdate(item) ? `Update → v${store.registryUpdate(item)}` : store.registryInstalled(item) ? "Reinstall" : "Install" }}
              </button>
            </div>
          </div>
        </li>
      </ul>

      <p v-else-if="!busy" class="mt-4 text-sm text-zinc-600">
        Nothing from mercs.ink yet — either the registry has no published mods or
        modkit could not reach it.
      </p>
    </section>

    <!-- ──────────────── Mod repositories — the curated registry.json set ──────────────── -->
    <section class="mt-10">
      <div class="flex items-end justify-between gap-4">
        <div>
          <h3 class="plate-label">Mod repositories</h3>
          <p class="mt-0.5 text-xs text-zinc-500">
            Mods indexed by curated GitHub repositories. Download → enable →
            deploy; state is reconciled against your game folder. This is where
            third-party plugins live — a mod like dxwrapper will never carry a
            Quartermaster manifest, so it can only ever appear here.
            <span v-if="catalogSource" class="text-zinc-600">(source: {{ catalogSource }})</span>
          </p>
        </div>
        <span class="shrink-0 text-xs text-zinc-600">{{ catalog.length }} mod(s)</span>
      </div>

    <ul v-if="catalog.length" class="mt-4 space-y-3">
      <li
        v-for="item in catalog"
        :key="`${item.repository}#${item.slug}`"
        class="guilloche rounded-xl border border-zinc-800 bg-zinc-900/50 p-4"
      >
        <div class="flex items-start justify-between gap-4">
          <div class="min-w-0">
            <div class="flex flex-wrap items-center gap-2">
              <p class="font-medium text-zinc-100">{{ item.name }}</p>
              <span
                class="stamp text-zinc-400"
                title="From a curated mod repository, not the mercs.ink registry"
                >repository</span
              >
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
                class="stamp text-amber-300"
                :title="`A newer release (v${store.catalogUpdate(item)}) is available — your Library copy is older`"
                >update available</span
              >
              <!-- Lifecycle state, reconciled against the game folder -->
              <span
                v-if="store.catalogModState(item) === 'deployed'"
                class="stamp text-violet-300"
                >deployed</span
              >
              <span
                v-else-if="store.catalogModState(item) === 'enabled'"
                class="stamp text-emerald-300"
                >enabled · not deployed</span
              >
              <span
                v-else-if="store.catalogModState(item) === 'downloaded'"
                class="stamp text-zinc-400"
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
              class="btn-plate"
              :disabled="busy"
              title="Download this mod's release asset(s) into the Library"
              @click="download(item)"
            >
              Download
            </button>
            <template v-else-if="store.catalogModState(item) === 'downloaded'">
              <span
                v-if="store.catalogModBlockedBy(item)"
                class="rounded-lg border border-red-700/50 bg-red-900/20 px-3 py-1.5 text-sm text-red-400"
                :title="`Incompatible with ${store.catalogModBlockedBy(item)!.name} (currently enabled) — disable it first`"
              >
                Incompatible with {{ store.catalogModBlockedBy(item)!.name }}
              </span>
              <button
                v-else
                class="rounded-lg bg-sky-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-500 disabled:opacity-50"
                :disabled="busy"
                title="Mark this mod for deployment"
                @click="enable(item)"
              >
                Enable
              </button>
            </template>
            <button
              v-else-if="store.catalogModState(item) === 'enabled'"
              class="btn-plate"
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
        class="empty-plate mt-4"
      >
        <p class="text-zinc-400">No repository mods.</p>
        <p class="mt-1 text-sm text-zinc-600">
          Add repository sources to
          <code class="text-zinc-400">registry.json</code>. Each repo lists its mods
          in <code class="text-zinc-400">repository.json</code> (objects with name,
          description, type, and release assets).
        </p>
      </div>
    </section>
  </div>
</template>
