<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref } from "vue";
import { storeToRefs } from "pinia";
import { open } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "../stores/project";

const store = useProjectStore();
const { gameInfo, gamePath, busy, gameRunning } = storeToRefs(store);

// In-flight launch/stop, so the button can't be double-fired.
const transitioning = ref(false);

// Arm pmc_blackbox's verbose log hooks for the next launch. Off by default —
// the per-line disk flush is too costly for regular play; opt in to diagnose.
const verboseLog = ref(false);

let pollTimer: ReturnType<typeof setInterval> | null = null;
onMounted(() => {
  store.refreshRunning();
  pollTimer = setInterval(() => store.refreshRunning(), 2500);
});
onBeforeUnmount(() => {
  if (pollTimer) clearInterval(pollTimer);
});

async function play() {
  if (transitioning.value || gameRunning.value) return;
  transitioning.value = true;
  try {
    await store.launchGame(null, verboseLog.value);
  } catch {
    /* surfaced via store.error */
  } finally {
    transitioning.value = false;
  }
}

async function stop() {
  if (transitioning.value) return;
  transitioning.value = true;
  try {
    await store.stopGame();
  } catch {
    /* surfaced via store.error */
  } finally {
    transitioning.value = false;
  }
}

async function chooseFolder() {
  const dir = await open({
    directory: true,
    title: "Select your Mercenaries 2 install folder",
  });
  if (typeof dir === "string") {
    await store.setGameFolder(dir).catch(() => {});
  }
}

function tail(p: string, n = 48): string {
  return p.length > n ? "…" + p.slice(p.length - n) : p;
}

function versionTone(v: string): string {
  if (v === "v1.1") return "bg-emerald-500/15 text-emerald-300";
  if (v === "v1.0") return "bg-sky-500/15 text-sky-300";
  return "bg-zinc-700/40 text-zinc-400";
}
</script>

<template>
  <header
    class="flex items-center gap-4 border-b border-zinc-800 bg-zinc-900/80 px-6 py-3 backdrop-blur"
  >
    <!-- No game set -->
    <template v-if="!gameInfo">
      <div class="flex-1">
        <p class="text-sm font-medium text-zinc-200">No game folder set</p>
        <p class="text-xs text-zinc-500">
          Point the modkit at your Mercenaries 2 install to enable building &amp;
          deploying.
        </p>
      </div>
      <button
        class="rounded-lg bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
        :disabled="busy"
        @click="chooseFolder"
      >
        Set game folder
      </button>
    </template>

    <!-- Game detected -->
    <template v-else>
      <div class="flex items-start gap-3">
        <div
          class="flex h-9 w-9 items-center justify-center rounded-lg bg-zinc-800 text-xs font-bold text-zinc-400"
        >
          M2
        </div>
        <div class="min-w-0">
          <p class="text-sm font-medium text-zinc-100">Mercenaries 2</p>
          <p class="font-mono text-[11px] text-zinc-500" :title="gamePath ?? ''">
            {{ tail(gameInfo.root) }}
          </p>
          <!-- Status badges live under the folder path so they don't crowd the
               launch controls; compact 10px chips that wrap as needed. -->
          <div class="mt-1 flex flex-wrap items-center gap-1.5">
            <span
              class="rounded-full px-1.5 py-0.5 text-[10px] font-medium"
              :class="versionTone(gameInfo.version)"
            >
              {{ gameInfo.version }}
            </span>
            <span
              v-if="gameInfo.variant !== 'unknown'"
              class="rounded-full bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-400"
            >
              {{ gameInfo.variant }}
            </span>
            <span
              class="chip"
              :class="gameInfo.has_pmc_bb ? 'chip-ok' : 'chip-off'"
              title="pmc_bb.dll — our ASI loader + SecuROM spoof"
            >
              {{
                gameInfo.has_pmc_bb
                  ? "pmc_bb.dll ✓ (ASI loader)"
                  : "pmc_bb.dll ✗ (ASI loader)"
              }}
            </span>
            <span
              v-if="
                gameInfo.asi_loader_proxy &&
                gameInfo.asi_loader_proxy !== 'pmc_bb.dll'
              "
              class="chip chip-off"
              :title="`Alternate ASI loader proxy: ${gameInfo.asi_loader_proxy}`"
            >
              alt loader: {{ gameInfo.asi_loader_proxy }}
            </span>
            <span
              v-if="gameInfo.deployed_asi.length"
              class="rounded-full bg-violet-500/15 px-1.5 py-0.5 text-[10px] text-violet-300"
              :title="gameInfo.deployed_asi.map((a) => a.rel_path).join('\n')"
            >
              {{ gameInfo.deployed_asi.length }} ASI plugin{{
                gameInfo.deployed_asi.length === 1 ? "" : "s"
              }}
            </span>
            <span
              v-if="gameInfo.deployed_patches.length"
              class="rounded-full bg-indigo-500/15 px-1.5 py-0.5 text-[10px] text-indigo-300"
              :title="gameInfo.deployed_patches.join('\n')"
            >
              {{ gameInfo.deployed_patches.length }} patch WAD{{
                gameInfo.deployed_patches.length === 1 ? "" : "s"
              }} deployed
            </span>
          </div>
        </div>
      </div>

      <div class="flex-1"></div>

      <div class="flex items-center gap-2">
        <span
          v-if="gameRunning"
          class="flex items-center gap-1.5 text-xs font-medium text-emerald-400"
          title="Mercenaries 2 is running (launched by modkit)"
        >
          <span class="h-2 w-2 animate-pulse rounded-full bg-emerald-400"></span>
          Running
        </span>
        <label
          v-if="!gameRunning"
          class="flex items-center gap-1.5 text-xs text-zinc-400 select-none"
          title="Arm pmc_blackbox's verbose Lua/engine log hooks for this launch. Off by default — the per-line disk flush is expensive, so reserve it for diagnostic runs."
        >
          <input
            type="checkbox"
            v-model="verboseLog"
            :disabled="busy || transitioning"
            class="accent-emerald-600"
          />
          Verbose log
        </label>
        <button
          v-if="!gameRunning"
          class="rounded-lg bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
          :disabled="busy || transitioning"
          :title="
            verboseLog
              ? 'Launch Mercenaries 2 with verbose pmc_blackbox logging'
              : 'Launch Mercenaries 2'
          "
          data-gamepad-play
          @click="play"
        >
          {{ transitioning ? "Launching…" : "▶ Play" }}
        </button>
        <button
          v-else
          class="rounded-lg bg-red-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-red-500 disabled:opacity-50"
          :disabled="transitioning"
          title="Stop the running game"
          data-gamepad-play
          @click="stop"
        >
          {{ transitioning ? "Stopping…" : "■ Stop" }}
        </button>
        <button
          class="rounded-md px-2 py-1 text-xs text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200 disabled:opacity-50"
          :disabled="busy"
          @click="store.refreshGame()"
        >
          Refresh
        </button>
        <button
          class="rounded-md px-2 py-1 text-xs text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200"
          @click="chooseFolder"
        >
          Change
        </button>
      </div>
    </template>
  </header>
</template>

<style scoped>
.chip {
  border-radius: 9999px;
  padding: 0.125rem 0.375rem;
  font-size: 0.625rem;
}
.chip-ok {
  background-color: rgb(16 185 129 / 0.15);
  color: rgb(110 231 183);
}
.chip-off {
  background-color: rgb(113 113 122 / 0.25);
  color: rgb(161 161 170);
}
</style>
