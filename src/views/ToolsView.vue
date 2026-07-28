<script setup lang="ts">
/**
 * Workshop Tools — the toolset published by the mercs2-wad-simulator release,
 * installed and kept current by modkit.
 *
 * These 11 binaries had no update path of their own: they ship as bare release
 * assets, none of them knows its own version, and none can replace itself. This
 * page is that update path. The whole toolset comes from a single release, so it
 * has ONE version — hence one banner at the top rather than a version per row.
 */
import { computed, onMounted, onUnmounted, ref } from "vue";
import { RouterLink } from "vue-router";
import { storeToRefs } from "pinia";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { openPath, openUrl } from "@tauri-apps/plugin-opener";
import { useProjectStore } from "../stores/project";
import type { ToolsetProgress } from "../types";
import Spinner from "../components/Spinner.vue";

const TOOLSET_REPO =
  "https://github.com/Mercenaries-Fan-Build/mercs2-wad-simulator";

const store = useProjectStore();
const { toolset, toolsetProgress, runningTools, gamePath, error } =
  storeToRefs(store);

const isRunning = (name: string) => runningTools.value.includes(name);

/**
 * Updating replaces the whole version directory, and on Windows a running
 * binary keeps its file locked — so an update with a tool open would half-apply
 * and leave the prune to fail. Block it while anything modkit started is alive.
 */
const anyRunning = computed(() => runningTools.value.length > 0);

// Which single tool's button is spinning. The store's `toolsetProgress` says an
// install is running; this says which row the user clicked, so only that row's
// button shows a spinner.
const pending = ref<string | null>(null);

let unlisten: UnlistenFn[] = [];
// Same cadence the game bar polls `is_game_running` at.
let pollTimer: ReturnType<typeof setInterval> | null = null;

onMounted(async () => {
  store.refreshToolset().catch(() => {});
  store.pollTools();
  pollTimer = setInterval(() => store.pollTools(), 2500);
  unlisten = [
    await listen<ToolsetProgress>("toolset-progress", (ev) => {
      store.toolsetProgress = ev.payload;
    }),
  ];
});
onUnmounted(() => {
  unlisten.forEach((u) => u());
  if (pollTimer) clearInterval(pollTimer);
});

const installed = computed(
  () => toolset.value?.tools.filter((t) => t.path) ?? []
);
const hasAny = computed(() => installed.value.length > 0);
const busy = computed(() => toolsetProgress.value !== null);

/**
 * The two windowed programs, then the nine command-line tools. Grouping by
 * "does modkit drive it" would put Smuggler, Byteswap, Load Probe and SecuROM
 * Unwrap under applications, which they are not — that stays a per-row badge.
 *
 * Same row markup for both groups.
 */
const groups = computed(() => [
  {
    label: "applications",
    hint: "Windowed programs — install, then launch them.",
    tools: toolset.value?.tools.filter((t) => t.windowed) ?? [],
  },
  {
    label: "command-line tools",
    hint: "Run from a terminal. Modkit invokes the flagged ones for you.",
    tools: toolset.value?.tools.filter((t) => !t.windowed) ?? [],
  },
]);

async function install(name: string) {
  pending.value = name;
  await store.installTools([name]);
  pending.value = null;
}

async function updateAll() {
  pending.value = null;
  await store.installTools([]);
}

async function remove(name: string) {
  pending.value = name;
  await store.uninstallTool(name);
  pending.value = null;
}

/**
 * Reveal the install folder. Wrapped rather than called inline: `openPath`
 * returns a promise, so an inline `@click` turns any failure into an unhandled
 * rejection the user never sees — which looks exactly like a dead button.
 */
async function openFolder() {
  if (!toolset.value) return;
  try {
    await openPath(toolset.value.dir);
  } catch (e) {
    store.error = `Could not open ${toolset.value.dir}: ${e}`;
  }
}

/** Same reasoning as openFolder — surface the failure instead of dropping it. */
async function openReleases() {
  try {
    await openUrl(`${TOOLSET_REPO}/releases`);
  } catch (e) {
    store.error = `Could not open the releases page: ${e}`;
  }
}

/** Any CLI tool installed? Nothing to put on PATH otherwise. */
const hasCliInstalled = computed(() =>
  installed.value.some((t) => !t.windowed)
);

async function openShell() {
  await store.openToolShell();
}

/** The terminal each platform will actually open, for the button label. */
const terminalName =
  navigator.userAgent.indexOf("Win") !== -1
    ? "PowerShell"
    : navigator.userAgent.indexOf("Mac") !== -1
      ? "Terminal"
      : "a terminal";

function formatSize(bytes: number | null): string {
  if (bytes == null) return "—";
  const mb = bytes / (1024 * 1024);
  return mb >= 1 ? `${mb.toFixed(1)} MB` : `${Math.round(bytes / 1024)} KB`;
}

/** Why a tool has no button, in plain terms. */
function unavailableReason(name: string): string {
  const engineBacked = name === "mercs2_workshop" || name === "mercs2_game";
  return engineBacked
    ? "64-bit only — the engine has no 32-bit build."
    : "No build published for this machine.";
}
</script>

<template>
  <div class="mx-auto max-w-3xl px-8 py-6">
    <header>
      <h2 class="plate-title text-xl">Workshop tools</h2>
      <p class="text-sm text-zinc-500">
        The Workshop and its companion command-line tools ship separately from
        modkit. Modkit keeps them installed and up to date for you — they cannot
        update themselves, so downloading one by hand means staying on that build
        forever.
      </p>
    </header>

    <p
      v-if="error"
      class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
    >
      {{ error }}
    </p>

    <!-- Toolset version. One release publishes all of these, so there is one
         version for the set rather than one per tool. -->
    <section class="guilloche mt-5 rounded-xl border border-zinc-800 p-5">
      <div class="flex items-center justify-between gap-3">
        <div>
          <h3 class="plate-title text-sm">Toolset version</h3>
          <p class="mt-1 flex items-center gap-2 text-xs text-zinc-500">
            <template v-if="hasAny">
              <span class="serial text-zinc-300">{{
                toolset?.installed_tag
              }}</span>
              <span v-if="toolset?.latest_tag && !toolset.update_available">
                — up to date
              </span>
              <span v-else-if="toolset?.update_available" class="text-emerald-400">
                — {{ toolset.latest_tag }} available
              </span>
              <span v-else>— latest unknown (offline)</span>
            </template>
            <template v-else>
              Nothing installed yet.
              <span v-if="toolset?.latest_tag" class="serial text-zinc-300">
                {{ toolset.latest_tag }}
              </span>
            </template>
          </p>
        </div>

        <div class="flex items-center gap-2">
          <button v-if="toolset" class="btn-outline" @click="openFolder">
            Open folder
          </button>
          <button
            class="rounded-lg px-3 py-2 text-sm text-zinc-300 hover:bg-zinc-800"
            :disabled="busy"
            @click="store.refreshToolset()"
          >
            Check now
          </button>
          <button
            v-if="toolset?.update_available"
            class="btn-plate"
            :disabled="busy || anyRunning"
            :title="
              anyRunning
                ? 'Stop the running tools first — updating replaces their files'
                : ''
            "
            @click="updateAll"
          >
            <Spinner v-if="busy && !pending" />
            Update all to {{ toolset.latest_tag }}
          </button>
        </div>
      </div>

      <!-- A 60 MB mercs2_game download should never look like a hang. -->
      <p
        v-if="busy && toolsetProgress?.label"
        class="mt-3 flex items-center gap-2 text-sm text-emerald-300"
      >
        <Spinner />
        Downloading {{ toolsetProgress.label }}
        <span class="serial text-xs text-zinc-500">
          {{ toolsetProgress.done + 1 }} / {{ toolsetProgress.total }}
        </span>
      </p>

      <p v-if="hasAny" class="mt-3 text-xs text-zinc-500">
        Updating replaces every installed tool at once — they are published
        together and are not tested in mixed versions.
        <span v-if="anyRunning" class="text-amber-400">
          Stop the running tools first.
        </span>
      </p>

      <p class="mt-2 text-xs text-zinc-600">
        Published by
        <button
          class="text-zinc-400 underline hover:text-zinc-200"
          @click="openReleases"
        >
          mercs2-wad-simulator
        </button>
      </p>
    </section>


    <!-- Both groups share the row: the "used by modkit" badge is the only
         difference, so the markup is written once. -->
    <section v-for="g in groups" :key="g.label" class="mt-5">
      <div class="flex items-end justify-between gap-3">
        <div>
          <h3 class="plate-label px-1 py-2">{{ g.label }}</h3>
          <p class="mb-2 px-1 text-xs text-zinc-600">{{ g.hint }}</p>
        </div>
        <!-- The CLI half is only usable if you know where these live; this
             hands you a shell that already does. -->
        <button
          v-if="g.label === 'command-line tools' && hasCliInstalled"
          class="btn-outline mb-2 shrink-0"
          :title="`Open ${terminalName} in your home folder with these tools on PATH`"
          @click="openShell"
        >
          Open {{ terminalName }} here
        </button>
      </div>
      <div class="space-y-2">
        <article
          v-for="t in g.tools"
          :key="t.name"
          class="flex items-center gap-4 rounded-xl border border-zinc-800 bg-zinc-900/40 px-4 py-3"
        >
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-2">
              <h4 class="truncate text-sm font-medium text-zinc-100">
                {{ t.label }}
              </h4>
              <span v-if="t.path && t.companion_ready" class="stamp">
                installed
              </span>
              <span
                v-if="isRunning(t.name)"
                class="flex items-center gap-1.5 rounded-full border border-emerald-600/30 bg-emerald-500/10 px-2 py-0.5 text-[10px] text-emerald-300"
                title="Started by modkit and still running"
              >
                <span
                  class="h-1.5 w-1.5 animate-pulse rounded-full bg-emerald-400"
                />
                running
              </span>
              <!-- Shown whether or not it is installed: the warning is about
                   what the thing IS, so it has to be readable before you
                   decide to download it. -->
              <span
                v-if="t.experimental"
                class="rounded-full border border-brass-500/40 bg-brass-500/10 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide text-brass-300"
                title="Not yet faithful to the retail game — for testing"
              >
                experimental
              </span>
              <span
                v-if="t.driven_by_modkit"
                class="rounded-full border border-sky-600/30 bg-sky-500/10 px-2 py-0.5 text-[10px] text-sky-300"
                title="Modkit runs this tool itself — Build &amp; Deploy needs it"
              >
                used by modkit
              </span>
            </div>
            <p class="mt-0.5 text-xs text-zinc-500">{{ t.blurb }}</p>

            <p v-if="!t.available" class="mt-1 text-xs text-amber-500/80">
              {{ unavailableReason(t.name) }}
            </p>

            <!-- The Workshop reads its reference data from a folder beside its
                 exe, so the binary alone is not a working install. -->
            <p
              v-else-if="t.path && !t.companion_ready"
              class="mt-1 text-xs text-amber-400"
            >
              Its <code class="text-amber-300">{{ t.companion_dir }}</code> data
              is missing — reinstall to finish the job.
            </p>
            <p
              v-else-if="!t.path && t.companion_dir"
              class="mt-1 text-xs text-zinc-600"
            >
              Installs with its <code>{{ t.companion_dir }}</code> reference data.
            </p>

            <!-- Say this before the click, not after a failed launch: off
                 Windows there is no registry key to fall back on. -->
            <p
              v-if="t.requires_game_dir && !gamePath"
              class="mt-1 text-xs text-amber-400"
            >
              Needs your Mercenaries 2 folder —
              <RouterLink to="/setup" class="underline">set it up</RouterLink>
              first.
            </p>
          </div>

          <span class="serial shrink-0 text-xs text-zinc-600">
            {{ formatSize(t.size) }}
          </span>

          <div class="flex shrink-0 items-center gap-2">
            <!-- Only the windowed apps can be opened, and only once their data
                 bundle is there — launching a Workshop without it fails
                 obscurely rather than usefully. Process-aware like the play
                 button: while it runs, the action is to stop it. -->
            <template v-if="t.windowed && t.path && t.companion_ready">
              <button
                v-if="isRunning(t.name)"
                class="btn-seal-red"
                :title="`Stop ${t.label}`"
                @click="store.stopTool(t.name)"
              >
                Stop
              </button>
              <button
                v-else
                class="btn-plate"
                :disabled="t.requires_game_dir && !gamePath"
                :title="
                  t.requires_game_dir && !gamePath
                    ? 'Set your Mercenaries 2 folder on the Setup page first'
                    : `Launch ${t.label}`
                "
                @click="store.launchTool(t.name)"
              >
                Open
              </button>
            </template>
            <button
              v-if="t.path && !t.companion_ready"
              class="btn-plate"
              :disabled="busy"
              @click="install(t.name)"
            >
              <Spinner v-if="pending === t.name" />
              Repair
            </button>
            <!-- Removing a running tool cannot work on Windows (its file is
                 locked) and is a surprise everywhere else. Stop it first. -->
            <button
              v-else-if="t.path"
              class="rounded-lg px-3 py-2 text-sm text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200 disabled:opacity-40 disabled:hover:bg-transparent"
              :disabled="busy || isRunning(t.name)"
              :title="isRunning(t.name) ? `Stop ${t.label} before removing it` : ''"
              @click="remove(t.name)"
            >
              Remove
            </button>
            <button
              v-else
              class="btn-plate"
              :disabled="!t.available || busy"
              @click="install(t.name)"
            >
              <Spinner v-if="pending === t.name" />
              Install
            </button>
          </div>
        </article>
      </div>
    </section>

    <p class="mt-6 text-xs text-zinc-600">
      Modkit installs these under its own cache folder and never touches your
      game install. Build tools are also fetched automatically the first time a
      modkit feature needs one.
    </p>
  </div>
</template>
