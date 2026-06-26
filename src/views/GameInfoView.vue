<script setup lang="ts">
import { computed, ref } from "vue";
import { storeToRefs } from "pinia";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useProjectStore } from "../stores/project";

const store = useProjectStore();
const { gameInfo, busy, error, pmcBbVersion, componentUpdates, vcRedist } =
  storeToRefs(store);

const pmcBbUpdate = computed(() => componentUpdates.value["pmc_bb"]);
const checking = ref(false);
const stage = ref("");
const pmcMsg = ref<string | null>(null);
const vcMsg = ref<string | null>(null);

function versionTone(v: string): string {
  if (v === "v1.1") return "bg-emerald-500/15 text-emerald-300";
  if (v === "v1.0") return "bg-sky-500/15 text-sky-300";
  return "bg-zinc-700/40 text-zinc-400";
}

function fmtBytes(n: number): string {
  if (!n) return "—";
  const mb = n / (1024 * 1024);
  return `${mb.toFixed(1)} MB (${n.toLocaleString()} bytes)`;
}

// Re-detect the install and re-check component releases together.
async function refreshAll() {
  checking.value = true;
  try {
    await Promise.all([
      store.refreshGame().catch(() => {}),
      store.checkComponentUpdates(),
    ]);
  } finally {
    checking.value = false;
  }
}

async function installPmcBb() {
  stage.value = "Downloading pmc_bb.dll…";
  pmcMsg.value = null;
  try {
    const res = await store.installPmcBb();
    pmcMsg.value = `Installed pmc_bb.dll ${res.version} → ${res.path}`;
  } catch {
    /* surfaced via store.error */
  } finally {
    stage.value = "";
  }
}

async function installVcRedist() {
  stage.value = "Downloading the Microsoft VC++ 2008 runtime… (approve the UAC prompt)";
  vcMsg.value = null;
  try {
    const res = await store.installVcRedist();
    vcMsg.value = res.message;
  } catch {
    /* surfaced via store.error */
  } finally {
    stage.value = "";
  }
}
</script>

<template>
  <div class="mx-auto max-w-3xl px-8 py-6">
    <header class="flex items-start justify-between gap-4">
      <div>
        <h2 class="text-xl font-semibold">Game Info</h2>
        <p class="text-sm text-zinc-500">
          What modkit detected about your install and the pmc_bb.dll ASI loader,
          plus any available updates.
        </p>
      </div>
      <button
        v-if="gameInfo"
        class="shrink-0 rounded-md border border-zinc-700 px-3 py-1.5 text-xs text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200 disabled:opacity-50"
        :disabled="busy || checking"
        @click="refreshAll"
      >
        {{ checking ? "Checking…" : "Refresh & check updates" }}
      </button>
    </header>

    <div
      v-if="!gameInfo"
      class="mt-10 rounded-xl border border-dashed border-zinc-800 px-8 py-16 text-center text-zinc-500"
    >
      Set your game folder in the bar above to see install details.
    </div>

    <template v-else>
      <div
        v-if="error"
        class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
      >
        {{ error }}
      </div>

      <!-- Readiness banner -->
      <div
        class="mt-5 rounded-xl border p-4"
        :class="
          store.gameFullySetUp
            ? 'border-emerald-500/30 bg-emerald-500/10'
            : 'border-amber-500/30 bg-amber-500/10'
        "
      >
        <p
          class="text-sm font-medium"
          :class="store.gameFullySetUp ? 'text-emerald-300' : 'text-amber-300'"
        >
          {{
            store.gameFullySetUp
              ? "Ready for modding ✓"
              : "Not fully set up for modding"
          }}
        </p>
        <p
          class="mt-1 text-sm"
          :class="
            store.gameFullySetUp ? 'text-emerald-300/80' : 'text-amber-300/80'
          "
        >
          <template v-if="store.gameFullySetUp">
            v1.1, cracked, and pmc_bb.dll installed.
          </template>
          <template v-else>
            Needs v1.1 + cracked exe + pmc_bb.dll. Finish in
            <RouterLink to="/setup" class="underline">Setup</RouterLink>.
          </template>
        </p>
        <p
          v-if="store.vcRedistMissing"
          class="mt-2 text-sm font-medium text-amber-300"
        >
          ⚠ The 32-bit Visual C++ 2008 runtime is missing — the game won't launch
          (“binkw32.dll was not found”) until it's installed below.
        </p>
      </div>

      <!-- Game details -->
      <section class="mt-4 rounded-xl border border-zinc-800 p-5">
        <h3 class="font-medium text-zinc-100">Game</h3>
        <dl class="mt-3 space-y-2 text-sm">
          <div class="flex items-center gap-3">
            <dt class="w-32 shrink-0 text-zinc-500">Version</dt>
            <dd>
              <span
                class="rounded-full px-2 py-0.5 text-xs font-medium"
                :class="versionTone(gameInfo.version)"
                >{{ gameInfo.version }}</span
              >
            </dd>
          </div>
          <div class="flex items-center gap-3">
            <dt class="w-32 shrink-0 text-zinc-500">Variant</dt>
            <dd class="text-zinc-300">{{ gameInfo.variant }}</dd>
          </div>
          <div class="flex gap-3">
            <dt class="w-32 shrink-0 text-zinc-500">Install folder</dt>
            <dd class="min-w-0 break-all font-mono text-xs text-zinc-300">
              {{ gameInfo.root }}
            </dd>
          </div>
          <div class="flex gap-3">
            <dt class="w-32 shrink-0 text-zinc-500">Executable</dt>
            <dd class="min-w-0 break-all font-mono text-xs text-zinc-300">
              {{ gameInfo.exe_path }}
            </dd>
          </div>
          <div class="flex gap-3">
            <dt class="w-32 shrink-0 text-zinc-500">Exe size</dt>
            <dd class="text-zinc-300">{{ fmtBytes(gameInfo.exe_size) }}</dd>
          </div>
          <div v-if="gameInfo.data_dir" class="flex gap-3">
            <dt class="w-32 shrink-0 text-zinc-500">Data dir</dt>
            <dd class="min-w-0 break-all font-mono text-xs text-zinc-300">
              {{ gameInfo.data_dir }}
            </dd>
          </div>
          <div class="flex gap-3">
            <dt class="w-32 shrink-0 text-zinc-500">Deployed</dt>
            <dd class="text-zinc-300">
              {{ gameInfo.deployed_asi.length }} ASI plugin{{
                gameInfo.deployed_asi.length === 1 ? "" : "s"
              }}
              ·
              {{ gameInfo.deployed_patches.length }} patch WAD{{
                gameInfo.deployed_patches.length === 1 ? "" : "s"
              }}
            </dd>
          </div>
        </dl>
      </section>

      <!-- pmc_bb.dll -->
      <section class="mt-4 rounded-xl border border-zinc-800 p-5">
        <div class="flex items-start justify-between gap-4">
          <div class="min-w-0">
            <h3 class="font-medium text-zinc-100">pmc_bb.dll (ASI loader)</h3>
            <p class="mt-1 text-sm text-zinc-400">
              Our ASI loader + SecuROM spoof — required to inject plugins.
            </p>

            <dl class="mt-3 space-y-2 text-sm">
              <div class="flex items-center gap-3">
                <dt class="w-32 shrink-0 text-zinc-500">Status</dt>
                <dd>
                  <span
                    class="rounded-full px-2 py-0.5 text-xs font-medium"
                    :class="
                      gameInfo.has_pmc_bb
                        ? 'bg-emerald-500/15 text-emerald-300'
                        : 'bg-zinc-700/40 text-zinc-400'
                    "
                    >{{ gameInfo.has_pmc_bb ? "Installed ✓" : "Not installed" }}</span
                  >
                </dd>
              </div>
              <div class="flex items-center gap-3">
                <dt class="w-32 shrink-0 text-zinc-500">Installed version</dt>
                <dd class="text-zinc-300">
                  {{
                    gameInfo.has_pmc_bb
                      ? (pmcBbVersion ?? "unknown (installed out-of-band)")
                      : "—"
                  }}
                </dd>
              </div>
              <div class="flex items-center gap-3">
                <dt class="w-32 shrink-0 text-zinc-500">Latest release</dt>
                <dd class="text-zinc-300">{{ pmcBbUpdate?.latest ?? "—" }}</dd>
              </div>
              <div
                v-if="
                  gameInfo.asi_loader_proxy &&
                  gameInfo.asi_loader_proxy !== 'pmc_bb.dll'
                "
                class="flex items-center gap-3"
              >
                <dt class="w-32 shrink-0 text-zinc-500">Alt loader</dt>
                <dd class="text-zinc-300">{{ gameInfo.asi_loader_proxy }}</dd>
              </div>
            </dl>

            <!-- Update notice -->
            <p
              v-if="pmcBbUpdate?.available"
              class="mt-3 flex items-center gap-1.5 text-sm font-medium text-amber-300"
            >
              <span class="h-1.5 w-1.5 rounded-full bg-amber-400" />
              Update available → {{ pmcBbUpdate.latest }}
              <button class="underline" @click="openUrl(pmcBbUpdate.url)">
                release notes
              </button>
            </p>
            <p
              v-else-if="gameInfo.has_pmc_bb && pmcBbVersion && pmcBbUpdate"
              class="mt-3 text-sm text-emerald-300/80"
            >
              Up to date ✓
            </p>
            <p
              v-else-if="gameInfo.has_pmc_bb && !pmcBbVersion"
              class="mt-3 text-xs text-zinc-500"
            >
              Version unknown — reinstall through modkit to start tracking
              updates.
            </p>

            <p
              v-if="pmcMsg"
              class="mt-3 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-300"
            >
              {{ pmcMsg }}
            </p>
            <p v-if="stage" class="mt-2 text-xs text-zinc-500">{{ stage }}</p>
          </div>

          <button
            class="shrink-0 rounded-lg px-3 py-2 text-sm font-medium disabled:opacity-50"
            :class="
              pmcBbUpdate?.available
                ? 'bg-amber-500 text-zinc-900 hover:bg-amber-400'
                : 'bg-emerald-600 text-white hover:bg-emerald-500'
            "
            :disabled="busy"
            @click="installPmcBb"
          >
            {{
              pmcBbUpdate?.available
                ? "Update"
                : gameInfo.has_pmc_bb
                  ? "Reinstall"
                  : "Install"
            }}
          </button>
        </div>
      </section>

      <!-- Microsoft Visual C++ 2008 runtime (host dependency) -->
      <section
        v-if="vcRedist?.applicable"
        class="mt-4 rounded-xl border p-5"
        :class="
          vcRedist.installed
            ? 'border-zinc-800'
            : 'border-amber-500/40 bg-amber-500/5'
        "
      >
        <div class="flex items-start justify-between gap-4">
          <div class="min-w-0">
            <h3 class="font-medium text-zinc-100">
              Microsoft Visual C++ 2008 runtime
            </h3>
            <p class="mt-1 text-sm text-zinc-400">
              The game and its binkw32.dll are 32-bit and need the VC++ 2008
              (x86) runtime. Without it, Windows can't load binkw32.dll and shows
              <span class="text-zinc-300"
                >“binkw32.dll was not found”</span
              >
              at launch.
            </p>

            <dl class="mt-3 space-y-2 text-sm">
              <div class="flex items-center gap-3">
                <dt class="w-32 shrink-0 text-zinc-500">Status</dt>
                <dd>
                  <span
                    class="rounded-full px-2 py-0.5 text-xs font-medium"
                    :class="
                      vcRedist.installed
                        ? 'bg-emerald-500/15 text-emerald-300'
                        : 'bg-amber-500/15 text-amber-300'
                    "
                    >{{
                      vcRedist.installed ? "Installed ✓" : "Not installed"
                    }}</span
                  >
                </dd>
              </div>
            </dl>

            <p
              v-if="!vcRedist.installed"
              class="mt-3 text-sm text-amber-300/90"
            >
              modkit will download the genuine Microsoft-signed installer,
              verify its signature, and run it (you'll see a Windows UAC prompt
              showing Microsoft as the publisher).
            </p>

            <p
              v-if="vcMsg"
              class="mt-3 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-300"
            >
              {{ vcMsg }}
            </p>
          </div>

          <button
            v-if="!vcRedist.installed"
            class="shrink-0 rounded-lg bg-amber-500 px-3 py-2 text-sm font-medium text-zinc-900 hover:bg-amber-400 disabled:opacity-50"
            :disabled="busy"
            @click="installVcRedist"
          >
            Install runtime
          </button>
        </div>
      </section>
    </template>
  </div>
</template>
