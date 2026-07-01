<script setup lang="ts">
import { ref, computed, watchEffect, onUnmounted } from "vue";
import { storeToRefs } from "pinia";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useProjectStore } from "../stores/project";
import type {
  LogReport,
  VerifyReport,
  GenerateManifestResult,
  DebugZipResult,
  HashProgress,
} from "../types";
import ProgressBar from "../components/ProgressBar.vue";
import Spinner from "../components/Spinner.vue";

const store = useProjectStore();
const { busy, error, gameInfo } = storeToRefs(store);

const logPath = ref<string | null>(null);
const report = ref<LogReport | null>(null);

// --- Verify game files ---
const verifying = ref(false);
const generating = ref(false);
const verifyReport = ref<VerifyReport | null>(null);
const genResult = ref<GenerateManifestResult | null>(null);
const progress = ref<HashProgress | null>(null);
// Latest textual phase ("Reading manifest…", "Inspecting blocks in …").
const statusMsg = ref<string | null>(null);
// Inline error for this section (separate from the shared store error).
const opError = ref<string | null>(null);
// A locally generated manifest, used to verify before it's published.
const localManifest = ref<string | null>(null);

// --- Build debug bundle ---
const building = ref(false);
const debugResult = ref<DebugZipResult | null>(null);
const debugError = ref<string | null>(null);
// Latest phase text streamed from the backend ("Verifying game files…", etc.).
const debugStatus = ref<string | null>(null);

// The maintainer card is for building the bundled manifest; pipeline release
// builds set VITE_RELEASE_BUILD, so it shows only in local/dev builds.
const isMaintainerBuild = import.meta.env.VITE_RELEASE_BUILD !== "true";

const progressPct = computed(() =>
  progress.value && progress.value.total > 0
    ? Math.round((progress.value.done / progress.value.total) * 100)
    : 0
);
// One-line status under the bar: phase text plus a count when we have one.
const statusLabel = computed(() => {
  const p = progress.value;
  const phase = statusMsg.value ?? "Working…";
  return p && p.total > 0
    ? `${phase} — ${p.done}/${p.total} (${progressPct.value}%)`
    : phase;
});
const verifyClean = computed(
  () =>
    !!verifyReport.value &&
    verifyReport.value.missing.length === 0 &&
    verifyReport.value.corrupt.length === 0
);

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const u = ["KB", "MB", "GB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(1)} ${u[i]}`;
}

// Stream numeric progress + textual status from the backend during a run.
let unlisten: UnlistenFn[] = [];
async function withProgress<T>(
  progressEvent: string,
  statusEvent: string,
  initialStatus: string,
  run: () => Promise<T>
): Promise<T> {
  progress.value = null; // indeterminate until the first numeric tick
  statusMsg.value = initialStatus;
  unlisten = [
    await listen<HashProgress>(progressEvent, (ev) => (progress.value = ev.payload)),
    await listen<string>(statusEvent, (ev) => (statusMsg.value = ev.payload)),
  ];
  try {
    return await run();
  } finally {
    unlisten.forEach((u) => u());
    unlisten = [];
    progress.value = null;
    statusMsg.value = null;
  }
}
onUnmounted(() => unlisten.forEach((u) => u()));

async function runVerify(useLocal = false) {
  verifying.value = true;
  verifyReport.value = null;
  opError.value = null;
  try {
    verifyReport.value = await withProgress(
      "verify-progress",
      "verify-status",
      "Starting…",
      () => store.verifyGame(useLocal ? (localManifest.value ?? undefined) : undefined)
    );
  } catch (e) {
    opError.value = String(e);
  } finally {
    verifying.value = false;
  }
}

async function pickAndVerify() {
  const f = await open({
    title: "Select a reference manifest",
    filters: [{ name: "Manifest", extensions: ["json"] }],
  });
  if (typeof f !== "string") return;
  localManifest.value = f;
  await runVerify(true);
}

async function generate() {
  generating.value = true;
  genResult.value = null;
  opError.value = null;
  try {
    genResult.value = await withProgress(
      "manifest-progress",
      "manifest-status",
      "Scanning install…",
      () => store.generateManifest()
    );
    localManifest.value = genResult.value.path;
  } catch (e) {
    opError.value = String(e);
  } finally {
    generating.value = false;
  }
}

// A filesystem-safe timestamp (YYYY-MM-DD-HHMMSS) for the default zip name.
function stamp(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return (
    `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}` +
    `-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`
  );
}

async function buildDebugBundle() {
  const dest = await save({
    title: "Save debug bundle",
    defaultPath: `mercs2-modkit-debug-${stamp()}.zip`,
    filters: [{ name: "Zip archive", extensions: ["zip"] }],
  });
  if (typeof dest !== "string") return;

  building.value = true;
  debugResult.value = null;
  debugError.value = null;
  debugStatus.value = "Starting…";
  const stop = await listen<string>(
    "debug-status",
    (ev) => (debugStatus.value = ev.payload)
  );
  try {
    debugResult.value = await store.buildDebugZip(dest);
  } catch (e) {
    debugError.value = String(e);
  } finally {
    stop();
    building.value = false;
    debugStatus.value = null;
  }
}

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
        Check your install is intact and analyze
        <code class="text-zinc-400">pmc_blackbox.log</code> to see how far the
        world-load got.
      </p>
    </header>

    <!-- Verify game files -->
    <section class="mt-5 rounded-xl border border-zinc-800 p-5">
      <h3 class="font-medium text-zinc-100">Verify game files</h3>
      <p class="mt-1 text-sm text-zinc-400">
        Hash every file in your install and compare it to a known-good baseline —
        catches a partial extraction or a damaged copy (e.g. a missing
        <code class="text-zinc-300">binkw32.dll</code>) before it turns into a
        cryptic launch error. Mods and modkit-managed files are ignored.
      </p>

      <div class="mt-4 flex flex-wrap items-center gap-2">
        <button
          class="inline-flex items-center gap-2 rounded-lg bg-emerald-600 px-4 py-2 text-sm font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
          :disabled="!gameInfo || verifying || generating"
          @click="runVerify(false)"
        >
          <Spinner v-if="verifying" />
          {{ verifying ? "Verifying…" : "Verify game files" }}
        </button>
        <button
          class="rounded-lg border border-zinc-700 px-3 py-2 text-sm text-zinc-300 hover:bg-zinc-800 disabled:opacity-50"
          :disabled="!gameInfo || verifying || generating"
          @click="pickAndVerify"
        >
          Use a local manifest…
        </button>
        <span v-if="!gameInfo" class="text-xs text-zinc-500">
          Set your game folder first.
        </span>
      </div>

      <ProgressBar
        v-if="verifying"
        :indeterminate="!progress?.total"
        :value="progressPct"
        :label="statusLabel"
        class="mt-4"
      />

      <p
        v-if="opError"
        class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
      >
        {{ opError }}
      </p>

      <!-- Verify report -->
      <template v-if="verifyReport && !verifying">
        <div
          class="mt-4 rounded-lg border px-4 py-3 text-sm"
          :class="
            verifyClean
              ? 'border-emerald-500/30 bg-emerald-500/10 text-emerald-300'
              : 'border-red-500/30 bg-red-500/10 text-red-300'
          "
        >
          <template v-if="verifyClean">
            All {{ verifyReport.ok }} vanilla files present and intact ✓
          </template>
          <template v-else>
            {{ verifyReport.missing.length }} missing ·
            {{ verifyReport.corrupt.length }} corrupt ·
            {{ verifyReport.ok }} OK
          </template>
        </div>

        <div v-if="verifyReport.missing.length" class="mt-3">
          <h4 class="text-sm font-medium text-red-300">
            Missing files ({{ verifyReport.missing.length }})
          </h4>
          <p class="mt-1 text-xs text-zinc-500">
            These vanilla files are absent from your install — a sign of a
            partial extraction or a damaged copy.
          </p>
          <ul
            class="mt-2 max-h-48 overflow-auto rounded-lg bg-black/40 p-3 font-mono text-xs text-red-300/90"
          >
            <li v-for="m in verifyReport.missing" :key="m">{{ m }}</li>
          </ul>
        </div>

        <div v-if="verifyReport.corrupt.length" class="mt-3">
          <h4 class="text-sm font-medium text-amber-300">
            Corrupt / changed ({{ verifyReport.corrupt.length }})
          </h4>
          <ul
            class="mt-2 max-h-48 overflow-auto rounded-lg bg-black/40 p-3 font-mono text-xs text-amber-300/90"
          >
            <li v-for="c in verifyReport.corrupt" :key="c.path">
              {{ c.path }} —
              <span class="text-zinc-500"
                >{{ fmtBytes(c.actual_size) }} on disk, expected
                {{ fmtBytes(c.expected_size) }}</span
              >
            </li>
          </ul>
        </div>

        <div
          v-for="w in verifyReport.wadDetails"
          :key="w.wad"
          class="mt-3 rounded-lg border border-amber-500/20 bg-amber-500/5 p-3"
        >
          <h4 class="font-mono text-sm font-medium text-amber-300">
            {{ w.wad }} — block-level
          </h4>
          <p
            v-if="!w.modified.length && !w.missing.length && !w.added.length"
            class="mt-1 text-xs text-emerald-300/80"
          >
            All block payloads intact — only the archive header/metadata differs
            (e.g. a re-saved WAD). Asset content is unchanged.
          </p>
          <p v-else class="mt-1 text-xs text-zinc-400">
            {{ w.modified.length }} modified · {{ w.missing.length }} missing ·
            {{ w.added.length }} added ·
            <span class="text-zinc-300"
              >{{ w.affectedAssets }} catalogued asset(s) affected</span
            >
          </p>
          <details v-if="w.modified.length || w.missing.length" class="mt-2">
            <summary
              class="cursor-pointer text-xs text-zinc-500 hover:text-zinc-300"
            >
              show changed blocks
            </summary>
            <ul
              class="mt-2 max-h-48 overflow-auto rounded-lg bg-black/40 p-3 font-mono text-xs"
            >
              <li v-for="b in w.modified" :key="'m' + b" class="text-amber-300/90">
                ~ {{ b }}
              </li>
              <li v-for="b in w.missing" :key="'x' + b" class="text-red-300/90">
                − {{ b }}
              </li>
            </ul>
          </details>
        </div>

        <div v-if="verifyReport.exes.length" class="mt-3">
          <h4 class="text-sm font-medium text-zinc-300">Executables</h4>
          <ul class="mt-2 space-y-2 text-xs">
            <li v-for="e in verifyReport.exes" :key="e.file">
              <p
                class="font-mono"
                :class="e.identifiedAs ? 'text-emerald-300/90' : 'text-amber-300/90'"
              >
                {{ e.file }} →
                <span v-if="e.identifiedAs">{{ e.identifiedAs }} ✓</span>
                <span v-else>unrecognized</span>
              </p>
              <p
                v-for="n in e.notes"
                :key="n"
                class="mt-0.5 pl-4 text-amber-300/80"
              >
                ⚠ {{ n }}
              </p>
            </li>
          </ul>
        </div>

        <details v-if="verifyReport.extra.length" class="mt-3">
          <summary class="cursor-pointer text-xs text-zinc-500 hover:text-zinc-300">
            {{ verifyReport.extra.length }} extra file(s) not in the manifest
            (mods / saves — expected)
          </summary>
          <ul
            class="mt-2 max-h-40 overflow-auto rounded-lg bg-black/40 p-3 font-mono text-xs text-zinc-500"
          >
            <li v-for="x in verifyReport.extra" :key="x">{{ x }}</li>
          </ul>
        </details>

        <p class="mt-3 text-xs text-zinc-600">
          baseline: {{ verifyReport.manifestSource }} ·
          {{ verifyReport.ignored }} excluded file(s) ignored (exe, caches,
          config, mods)
        </p>
      </template>

      <!-- Maintainer: generate a reference manifest (local/dev builds only) -->
      <div
        v-if="isMaintainerBuild"
        class="mt-5 rounded-lg border border-zinc-800/80 bg-zinc-900/40 p-4"
      >
        <h4 class="text-sm font-medium text-zinc-300">
          Reference manifest (maintainer)
        </h4>
        <p class="mt-1 text-xs text-zinc-500">
          The canonical "known-good versions" manifest is
          <strong>bundled with the app</strong>. To update it, run the
          <code class="text-zinc-400">gen_manifest</code> example against your
          reference files, commit
          <code class="text-zinc-400">manifests/mercs2.manifest.json</code>, and
          rebuild. This button writes a copy to app data from a
          <em>fresh, never-launched, unmodded</em> install for local testing —
          then use “Verify against it”.
        </p>
        <div class="mt-3 flex flex-wrap items-center gap-2">
          <button
            class="inline-flex items-center gap-2 rounded-lg border border-zinc-700 px-3 py-2 text-xs text-zinc-300 hover:bg-zinc-800 disabled:opacity-50"
            :disabled="!gameInfo || verifying || generating"
            @click="generate"
          >
            <Spinner v-if="generating" :size="14" />
            {{ generating ? "Hashing…" : "Generate from this install" }}
          </button>
          <button
            v-if="localManifest"
            class="rounded-lg border border-zinc-700 px-3 py-2 text-xs text-zinc-300 hover:bg-zinc-800 disabled:opacity-50"
            :disabled="verifying || generating"
            @click="runVerify(true)"
          >
            Verify against it
          </button>
        </div>
        <ProgressBar
          v-if="generating"
          :indeterminate="!progress?.total"
          :value="progressPct"
          :label="statusLabel"
          class="mt-3"
        />
        <p
          v-if="genResult"
          class="mt-3 flex flex-wrap items-center gap-2 text-xs text-emerald-300"
        >
          Wrote {{ genResult.fileCount }} files
          ({{ fmtBytes(genResult.totalBytes) }}) →
          <button class="underline" @click="openPath(genResult.path)">
            {{ genResult.path }}
          </button>
        </p>
      </div>
    </section>

    <!-- Build debug bundle -->
    <section class="mt-5 rounded-xl border border-zinc-800 p-5">
      <h3 class="font-medium text-zinc-100">Build debug bundle</h3>
      <p class="mt-1 text-sm text-zinc-400">
        Package everything a maintainer needs to diagnose a problem into one
        dated <code class="text-zinc-300">.zip</code>: your game
        <code class="text-zinc-300">.log</code> files, a list of installed mods,
        the versions of everything (modkit, game, ASI loader, crack, VC++
        runtime), and a fresh file-integrity check.
      </p>

      <div class="mt-4 flex flex-wrap items-center gap-2">
        <button
          class="inline-flex items-center gap-2 rounded-lg bg-emerald-600 px-4 py-2 text-sm font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
          :disabled="!gameInfo || building"
          @click="buildDebugBundle"
        >
          <Spinner v-if="building" />
          {{ building ? "Building…" : "Build debug zip" }}
        </button>
        <span v-if="!gameInfo" class="text-xs text-zinc-500">
          Set your game folder first.
        </span>
        <span v-else-if="building && debugStatus" class="text-xs text-zinc-500">
          {{ debugStatus }}
        </span>
      </div>

      <p
        v-if="debugError"
        class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
      >
        {{ debugError }}
      </p>

      <div
        v-if="debugResult && !building"
        class="mt-4 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-4 py-3 text-sm text-emerald-300"
      >
        <p>
          Bundled {{ debugResult.logCount }} log file(s) +
          {{ debugResult.integrityOk ? "a clean" : "a flagged" }} integrity
          report ({{ fmtBytes(debugResult.bytes) }}).
        </p>
        <p class="mt-1 flex flex-wrap items-center gap-3">
          <button class="underline" @click="revealItemInDir(debugResult.path)">
            Show in folder
          </button>
          <button class="underline" @click="openPath(debugResult.path)">
            Open
          </button>
        </p>
        <ul
          v-if="debugResult.notes.length"
          class="mt-2 list-disc pl-5 text-xs text-amber-300/90"
        >
          <li v-for="n in debugResult.notes" :key="n">{{ n }}</li>
        </ul>
      </div>
    </section>

    <hr class="mt-6 border-zinc-800" />

    <h3 class="mt-6 font-medium text-zinc-100">Crash log analysis</h3>
    <p class="text-sm text-zinc-500">
      Analyze <code class="text-zinc-400">pmc_blackbox.log</code> to see how far
      the world-load got and classify the end-state.
    </p>

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
