<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { storeToRefs } from "pinia";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { openPath } from "@tauri-apps/plugin-opener";
import { useProjectStore } from "../stores/project";
import type { EnvVar } from "../types";
import ProgressBar from "./ProgressBar.vue";

// How the game runs on a non-Windows host. macOS: the modkit-managed Wine build
// (install / select / remove). Linux: which installed Proton. Both: the Wine
// environment list. Everything else in runtime.json (DLL overrides, registry,
// WINEDEBUG, game arguments, paths) is edited by hand in the file — this panel
// only says which of those are in effect.

const store = useProjectStore();
const { runtimeInfo, runtimeSettings, wineStatus, runtimeErrors, busy } = storeToRefs(store);
const loaded = ref(false);

const host = computed(() => runtimeInfo.value?.host ?? null);
const settings = computed(() => runtimeSettings.value?.settings ?? null);

// The env list is edited as a draft and saved explicitly, so a half-typed key
// is never written (the backend rejects invalid keys).
const draft = ref<EnvVar[]>([]);
watch(
  () => settings.value?.env,
  (env) => {
    draft.value = (env ?? []).map((v) => ({ ...v }));
  },
  { immediate: true },
);
const envDirty = computed(
  () => JSON.stringify(draft.value) !== JSON.stringify(settings.value?.env ?? []),
);

function addEnv() {
  draft.value.push({ key: "", value: "" });
}
function removeEnv(i: number) {
  draft.value.splice(i, 1);
}
async function saveEnv() {
  await store.setRuntimeEnv(draft.value).catch(() => {});
}

// Fields only the file sets, listed so it is visible that they are in effect.
const fileOnly = computed(() => {
  const s = settings.value;
  if (!s) return [];
  const out: string[] = [];
  const dlls = Object.keys(s.dllOverrides).length;
  if (dlls) out.push(`${dlls} DLL override${dlls === 1 ? "" : "s"}`);
  if (s.registry.length)
    out.push(`${s.registry.length} registry value${s.registry.length === 1 ? "" : "s"}`);
  if (s.winedebug) out.push(`WINEDEBUG=${s.winedebug}`);
  if (s.exeArgs.length) out.push(`game args: ${s.exeArgs.join(" ")}`);
  if (s.prefix) out.push(`prefix: ${s.prefix}`);
  return out;
});

async function revealSettings() {
  const p = runtimeSettings.value?.path;
  if (!p) return;
  await openPath(p.replace(/[\\/][^\\/]*$/, "")).catch((e) => {
    store.error = String(e);
  });
}

// --- macOS Wine builds -------------------------------------------------------

const installing = ref<string | null>(null);
const progress = ref<{ done: number; total: number | null } | null>(null);
let unlisten: UnlistenFn | null = null;

onMounted(async () => {
  unlisten = await listen<{
    key: string;
    doneBytes: number;
    totalBytes: number | null;
    done: boolean;
  }>("download-progress", (e) => {
    if (e.payload.key !== "wine") return;
    progress.value = { done: e.payload.doneBytes, total: e.payload.totalBytes };
  });
  await store.loadRuntime(true);
  loaded.value = true;
});
onUnmounted(() => unlisten?.());

const installedTags = computed(
  () => new Set((wineStatus.value?.installed ?? []).map((w) => w.tag)),
);

async function install(tag: string) {
  installing.value = tag;
  progress.value = null;
  try {
    await store.installWine(tag);
  } catch {
    /* surfaced via store.error */
  } finally {
    installing.value = null;
    progress.value = null;
  }
}

const progressLabel = computed(() => {
  const p = progress.value;
  if (!p) return `Fetching Wine ${installing.value}…`;
  const mb = (n: number) => (n / 1024 / 1024).toFixed(0);
  if (p.total && p.done >= p.total) return `Unpacking Wine ${installing.value}…`;
  return p.total
    ? `Downloading Wine ${installing.value} — ${mb(p.done)} / ${mb(p.total)} MB`
    : `Downloading Wine ${installing.value} — ${mb(p.done)} MB`;
});
const progressPct = computed(() => {
  const p = progress.value;
  return p?.total ? (p.done / p.total) * 100 : undefined;
});

function formatSize(n: number | null): string {
  return n == null ? "" : `${(n / 1024 / 1024).toFixed(0)} MB`;
}

// --- Linux Proton -------------------------------------------------------------

const protonChoice = computed({
  get: () => settings.value?.proton ?? "",
  set: (v: string) => {
    store.selectProton(v === "" ? null : v).catch(() => {});
  },
});
</script>

<template>
  <!-- Hidden only once the host is known to be Windows. While loading, or when
       loading failed, the panel still renders and says so. -->
  <section v-if="host !== 'windows'" class="guilloche rounded-xl border border-zinc-800 p-5">
    <h3 class="plate-title text-sm">Runtime (Wine / Proton)</h3>
    <ul v-if="runtimeErrors.length" class="mt-2 space-y-1">
      <li
        v-for="e in runtimeErrors"
        :key="e"
        class="whitespace-pre-wrap rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-200"
      >
        {{ e }}
      </li>
    </ul>
    <p v-if="!loaded" class="mt-1 text-sm text-zinc-500">Loading runtime settings…</p>
    <p v-else-if="!host" class="mt-1 text-sm text-zinc-400">
      The runtime could not be resolved, so there is nothing to configure until the error above
      is fixed.
    </p>
    <p v-else class="mt-1 text-sm text-zinc-400">
      <template v-if="host === 'macos'">
        The game runs under a Wine build modkit downloads and manages, from wherever the game
        folder is.
      </template>
      <template v-else>
        The game runs through Steam's Proton. Pick which installed Proton build to use.
      </template>
    </p>

    <dl class="mt-3 space-y-2 text-sm">
      <div v-if="host === 'macos'" class="flex items-center gap-3">
        <dt class="w-32 shrink-0 text-zinc-500">Wine</dt>
        <dd class="min-w-0 break-all font-mono text-xs text-zinc-300">
          {{ runtimeInfo?.wine ?? "—" }}
        </dd>
      </div>
      <div v-if="host === 'linux'" class="flex items-center gap-3">
        <dt class="w-32 shrink-0 text-zinc-500">Proton</dt>
        <dd class="min-w-0 flex-1">
          <select
            v-model="protonChoice"
            class="w-full rounded-md border border-zinc-700 bg-zinc-900 px-2 py-1 font-mono text-xs text-zinc-200"
          >
            <option value="">Automatic (MERCS2_PROTON, else first found)</option>
            <option
              v-if="settings?.proton && !runtimeInfo?.protons.includes(settings.proton)"
              :value="settings.proton"
            >
              {{ settings.proton }} (not found)
            </option>
            <option v-for="p in runtimeInfo?.protons ?? []" :key="p" :value="p">
              {{ p }}
            </option>
          </select>
          <p class="mt-1 font-mono text-[11px] text-zinc-500">
            Using: {{ runtimeInfo?.proton ?? "—" }}
          </p>
        </dd>
      </div>
      <div class="flex items-center gap-3">
        <dt class="w-32 shrink-0 text-zinc-500">Prefix</dt>
        <dd class="min-w-0 break-all font-mono text-xs text-zinc-300">
          {{ runtimeInfo?.prefix ?? "—" }}
        </dd>
      </div>
    </dl>

    <ul v-if="runtimeInfo?.notes.length" class="mt-3 space-y-1">
      <li
        v-for="n in runtimeInfo.notes"
        :key="n"
        class="whitespace-pre-wrap rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-200"
      >
        {{ n }}
      </li>
    </ul>

    <!-- macOS: Wine builds -->
    <div v-if="host === 'macos' && wineStatus" class="mt-4">
      <h4 class="plate-label">Wine builds</h4>
      <p class="mt-1 text-xs text-zinc-500">From {{ wineStatus.repo }}.</p>

      <ul class="mt-2 space-y-1.5">
        <li
          v-for="w in wineStatus.installed"
          :key="w.tag"
          class="flex items-center gap-2 rounded-lg border border-zinc-800 px-3 py-2 text-sm"
        >
          <span class="font-mono text-zinc-200">{{ w.tag }}</span>
          <span v-if="w.selected" class="stamp text-emerald-300">In use</span>
          <span v-if="!w.present" class="stamp text-red-300">Files missing</span>
          <span v-else-if="w.modified" class="stamp text-amber-300">Modified</span>
          <span class="flex-1" />
          <button
            v-if="!w.selected && w.present"
            class="rounded-md px-2 py-1 text-xs text-zinc-300 hover:bg-zinc-800 disabled:opacity-50"
            :disabled="busy"
            @click="store.selectWine(w.tag).catch(() => {})"
          >
            Use
          </button>
          <button
            class="rounded-md px-2 py-1 text-xs text-zinc-400 hover:bg-zinc-800 disabled:opacity-50"
            :disabled="busy"
            @click="store.removeWine(w.tag).catch(() => {})"
          >
            Remove
          </button>
        </li>
        <li v-if="!wineStatus.installed.length" class="text-xs text-zinc-500">
          No Wine build installed yet.
        </li>
      </ul>

      <p v-if="wineStatus.remoteError" class="mt-2 text-xs text-amber-300">
        Could not list published builds: {{ wineStatus.remoteError }}
      </p>
      <ul v-if="wineStatus.releases.length" class="mt-3 space-y-1.5">
        <li
          v-for="r in wineStatus.releases"
          :key="r.tag"
          class="flex items-center gap-2 text-sm"
        >
          <span class="font-mono text-zinc-300">{{ r.tag }}</span>
          <span class="text-xs text-zinc-500">{{ r.asset }} · {{ formatSize(r.size) }}</span>
          <span v-if="!r.ready" class="text-xs text-amber-300">still uploading</span>
          <span class="flex-1" />
          <button
            class="rounded-lg bg-emerald-600 px-3 py-1 text-xs font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
            :disabled="busy || !r.ready"
            @click="install(r.tag)"
          >
            {{ installedTags.has(r.tag) ? "Reinstall" : "Install" }}
          </button>
        </li>
      </ul>
      <ProgressBar
        v-if="installing"
        class="mt-3"
        :indeterminate="progressPct === undefined || progressPct >= 100"
        :value="progressPct"
        :label="progressLabel"
      />
    </div>

    <!-- Environment -->
    <div v-if="settings" class="mt-4">
      <h4 class="plate-label">Environment variables</h4>
      <p class="mt-1 text-xs text-zinc-500">
        Passed to {{ host === "macos" ? "Wine" : "Proton" }} on every launch.
      </p>
      <div v-for="(v, i) in draft" :key="i" class="mt-2 flex items-center gap-2">
        <input
          v-model="v.key"
          placeholder="NAME"
          class="w-56 rounded-md border border-zinc-700 bg-zinc-900 px-2 py-1 font-mono text-xs text-zinc-200"
        />
        <span class="text-zinc-500">=</span>
        <input
          v-model="v.value"
          placeholder="value"
          class="min-w-0 flex-1 rounded-md border border-zinc-700 bg-zinc-900 px-2 py-1 font-mono text-xs text-zinc-200"
        />
        <button
          class="rounded-md px-2 py-1 text-xs text-zinc-400 hover:bg-zinc-800"
          @click="removeEnv(i)"
        >
          ✕
        </button>
      </div>
      <div class="mt-2 flex items-center gap-2">
        <button
          class="rounded-md px-2 py-1 text-xs text-zinc-300 hover:bg-zinc-800"
          @click="addEnv"
        >
          + Add variable
        </button>
        <span class="flex-1" />
        <button
          class="rounded-lg bg-emerald-600 px-3 py-1 text-xs font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
          :disabled="!envDirty || busy"
          @click="saveEnv"
        >
          Save
        </button>
      </div>
    </div>

    <!-- The rest of runtime.json -->
    <div v-if="runtimeSettings" class="mt-4 text-xs text-zinc-500">
      <p>
        DLL overrides, registry values, WINEDEBUG, game arguments and paths are set by editing
        <button class="font-mono text-zinc-300 underline" @click="revealSettings">
          {{ runtimeSettings.path }}
        </button>
        by hand.
      </p>
      <p v-if="fileOnly.length" class="mt-1 text-zinc-400">
        In effect from the file: {{ fileOnly.join(" · ") }}
      </p>
    </div>
  </section>
</template>
