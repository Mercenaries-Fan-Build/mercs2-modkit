<script setup lang="ts">
import { ref, watchEffect } from "vue";
import { storeToRefs } from "pinia";
import { open } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "../stores/project";
import type { LogReport } from "../types";
import ProgressBar from "../components/ProgressBar.vue";

const store = useProjectStore();
const { busy, error, gameInfo } = storeToRefs(store);

const logPath = ref<string | null>(null);
const report = ref<LogReport | null>(null);

// Use the log discovered during game detection (reactive to folder changes).
watchEffect(() => {
  if (!logPath.value && gameInfo.value?.log_path) {
    logPath.value = gameInfo.value.log_path;
  }
});

async function pickLog() {
  const f = await open({
    title: "Select pmc_blackbox.log",
    filters: [
      { name: "Log", extensions: ["log", "txt"] },
      { name: "All files", extensions: ["*"] },
    ],
  });
  if (typeof f === "string") logPath.value = f;
}

async function analyze() {
  if (!logPath.value) return;
  report.value = null;
  report.value = await store.analyzeLog(logPath.value).catch(() => null);
}

function hex(n: number): string {
  return "0x" + (n >>> 0).toString(16).toUpperCase().padStart(8, "0");
}
function secs(ms: number): string {
  return `${(ms / 1000).toFixed(1)}s`;
}
function verdictTone(kind: string): string {
  return (
    {
      ReachedWorld: "bg-emerald-500/15 text-emerald-300 border-emerald-500/30",
      Crash: "bg-red-500/15 text-red-300 border-red-500/30",
      Hang: "bg-amber-500/15 text-amber-300 border-amber-500/30",
      Truncated: "bg-zinc-700/30 text-zinc-300 border-zinc-700",
    }[kind] ?? "bg-zinc-700/30 text-zinc-300 border-zinc-700"
  );
}
</script>

<template>
  <div class="mx-auto max-w-3xl px-8 py-6">
    <header>
      <h2 class="text-xl font-semibold">Diagnostics</h2>
      <p class="text-sm text-zinc-500">
        Analyze <code class="text-zinc-400">pmc_blackbox.log</code> to see how far
        the world-load got and classify the end-state.
      </p>
    </header>

    <!-- Log picker -->
    <div class="mt-5 flex gap-2">
      <input
        :value="logPath ?? ''"
        readonly
        :placeholder="
          gameInfo ? 'No pmc_blackbox.log found — browse for one' : 'Browse for a log…'
        "
        class="flex-1 rounded-lg border border-zinc-700 bg-zinc-900 px-3 py-2 text-sm"
      />
      <button
        class="rounded-lg border border-zinc-700 px-3 py-2 text-sm text-zinc-300 hover:bg-zinc-800"
        @click="pickLog"
      >
        Browse
      </button>
      <button
        class="rounded-lg bg-emerald-600 px-4 py-2 text-sm font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
        :disabled="busy || !logPath"
        @click="analyze"
      >
        Analyze
      </button>
    </div>

    <ProgressBar v-if="busy" indeterminate label="Analyzing…" class="mt-4" />
    <div
      v-if="error"
      class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
    >
      {{ error }}
    </div>

    <template v-if="report">
      <!-- Verdict + progress -->
      <div
        class="mt-6 rounded-xl border p-4"
        :class="verdictTone(report.verdict.kind)"
      >
        <div class="flex items-center justify-between">
          <span class="text-lg font-semibold">{{ report.verdict.kind }}</span>
          <span class="text-sm">{{ report.pct }}% · {{ report.furthest_name }}</span>
        </div>
        <div class="mt-2">
          <ProgressBar :value="report.pct" />
        </div>
        <p class="mt-2 text-xs opacity-80">
          phase {{ report.furthest_idx }} · {{ report.records }} records ·
          {{ secs(report.wall_ms) }} wall
          <span v-if="report.verdict.kind === 'Crash'">
            · EIP {{ hex(report.verdict.eip) }}
            <span v-if="report.verdict.label"> ({{ report.verdict.label }})</span>
          </span>
          <span v-else-if="report.verdict.kind === 'Hang'">
            · stuck {{ secs(report.verdict.stuck_ms) }}
          </span>
        </p>
      </div>

      <!-- Crash detail -->
      <section
        v-if="report.crash"
        class="mt-4 rounded-xl border border-red-500/20 bg-red-500/5 p-4"
      >
        <h3 class="text-sm font-medium text-red-300">Crash block</h3>
        <p class="mt-1 text-xs text-zinc-400">
          {{ report.crash.code }} @ {{ hex(report.crash.eip) }}
          <span v-if="report.crash.eip_label"> · {{ report.crash.eip_label }}</span>
          <span v-if="report.crash.av"> · AV {{ report.crash.av }}</span>
          <span v-if="report.crash.terminal"> · terminal</span>
        </p>
        <pre
          class="mt-2 max-h-48 overflow-auto rounded-lg bg-black/40 p-3 text-xs text-zinc-400"
        >{{ report.crash.block.join("\n") }}</pre>
      </section>

      <!-- Last activity / tail -->
      <section class="mt-4 rounded-xl border border-zinc-800 p-4">
        <h3 class="text-sm font-medium text-zinc-300">Last activity</h3>
        <p class="mt-1 text-xs text-zinc-500">
          last progress @ {{ report.last_progress_ts }} —
          {{ report.last_progress_msg || "—" }}
        </p>
        <pre
          class="mt-2 max-h-48 overflow-auto rounded-lg bg-black/40 p-3 text-xs text-zinc-400"
        >{{ report.tail.join("\n") }}</pre>
      </section>

      <!-- Build attribution -->
      <section v-if="report.build.length" class="mt-4 rounded-xl border border-zinc-800 p-4">
        <h3 class="text-sm font-medium text-zinc-300">Build attribution</h3>
        <ul class="mt-2 space-y-1 text-xs">
          <li v-for="b in report.build" :key="b.name" class="font-mono text-zinc-400">
            <span class="text-zinc-300">{{ b.kind }}</span> {{ b.name }} ·
            {{ b.hash_type }}={{ b.sha256.slice(0, 12) }}…
          </li>
        </ul>
      </section>

      <!-- Robustness -->
      <p class="mt-4 text-xs text-zinc-600">
        {{ report.unparsed_lines }} unparsed line{{
          report.unparsed_lines === 1 ? "" : "s"
        }}
        <span v-if="report.unknown_sources.length">
          · unknown sources:
          {{ report.unknown_sources.map((s) => s[0]).join(", ") }}
        </span>
      </p>
    </template>
  </div>
</template>
