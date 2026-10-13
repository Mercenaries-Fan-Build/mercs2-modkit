<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { storeToRefs } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "../stores/project";
import type { ComponentRemoval, PmcBbChoice, PmcBbVariant } from "../types";
import CrackPanel from "../components/CrackPanel.vue";
import ExeUpdatePanel from "../components/ExeUpdatePanel.vue";
import RuntimePanel from "../components/RuntimePanel.vue";
import ErrorBanner from "../components/ErrorBanner.vue";

const store = useProjectStore();
const {
  gameInfo,
  busy,
  pmcBbVersion,
  pmcBbAsset,
  pmcBbModified,
  dxwrapperVersion,
  componentUpdates,
  vcRedist,
  region,
} = storeToRefs(store);

const pmcBbUpdate = computed(() => componentUpdates.value["pmc_bb"]);
const dxwrapperUpdate = computed(() => componentUpdates.value["dxwrapper"]);

// Why a newer release isn't being offered, when there is one — either it is still
// uploading, or it no longer publishes the build this install uses. Shown so the
// gap between "a new tag exists" and "you can have it" is legible rather than
// looking like modkit failed to notice.
const pmcBbPending = computed(
  () => store.managedComponent("pmc_bb")?.pendingReason ?? null,
);

// The pmc_bb build the launched exe calls for, and the one this install should
// have (the pinned build, else that one) — both from the launch plan.
const recommended = computed<PmcBbChoice | null>(() => gameInfo.value?.launch?.pmc_bb_auto ?? null);
const wanted = computed<PmcBbChoice | null>(() => gameInfo.value?.launch?.pmc_bb_wanted ?? null);
const variants = ref<PmcBbVariant[]>([]);
const showVariants = ref(false);
const chosenVariant = ref("");

watch(
  () => (wanted.value?.overridden ? wanted.value.asset : ""),
  (v) => (chosenVariant.value = v),
  { immediate: true },
);

/** The installed build differs from what this install should have. */
const variantMismatch = computed(
  () => !!pmcBbAsset.value && !!wanted.value && pmcBbAsset.value !== wanted.value.asset,
);

async function loadPmcBbVariants() {
  if (variants.value.length) return;
  try {
    variants.value = await invoke<PmcBbVariant[]>("pmc_bb_variants");
  } catch {
    /* leave the advanced picker empty */
  }
}

watch(() => gameInfo.value?.root, loadPmcBbVariants, { immediate: true });

const pinnedExe = computed(() =>
  gameInfo.value?.launch?.selection === "override" ? gameInfo.value.launch.exe.path : null,
);

async function pinExe(path: string | null) {
  await store.setExeOverride(path).catch(() => {});
}

async function browseExe() {
  const f = await open({
    title: "Pin an exe to launch",
    defaultPath: gameInfo.value?.root,
    filters: [{ name: "Executable", extensions: ["exe"] }],
  });
  if (typeof f === "string") await pinExe(f);
}

function featureList(f: { crack: boolean; asi: boolean; log: boolean }): string {
  const on = [f.crack && "SecuROM spoof", f.asi && "ASI loader", f.log && "logging"];
  return on.filter(Boolean).join(" · ");
}
const checking = ref(false);
const stage = ref("");
const pmcMsg = ref<string | null>(null);
const dxMsg = ref<string | null>(null);
const vcMsg = ref<string | null>(null);
const regionMsg = ref<string | null>(null);

// Friendly names for the Region values the game recognizes (see
// docs/mercs2_install_registry_contract.md §1).
const REGION_LABELS: Record<string, string> = {
  mercenaries2_na: "North America",
  mercenaries2_enru: "EU — UK, France, Russia",
  mercenaries2_esit: "EU — Spain, Italy",
};

function regionLabel(r: string): string {
  return REGION_LABELS[r] ?? r;
}

function onRegionSelect(event: Event) {
  regionMsg.value = null;
  store.setPreferredRegion((event.target as HTMLSelectElement).value);
}

function versionTone(v: string): string {
  if (v === "v1.1") return "text-emerald-300";
  if (v === "v1.0") return "text-sky-300";
  return "text-zinc-500";
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
      store.refreshManaged(),
    ]);
  } finally {
    checking.value = false;
  }
}

async function installPmcBb() {
  const target = chosenVariant.value || recommended.value?.asset || "pmc_bb";
  stage.value = `Downloading ${target}…`;
  pmcMsg.value = null;
  try {
    const pinned = wanted.value?.overridden ? wanted.value.asset : "";
    if (chosenVariant.value !== pinned) {
      await store.setPmcBbOverride(chosenVariant.value || null);
    }
    const res = await store.installPmcBb();
    pmcMsg.value = `Installed ${res.asset} ${res.version} as pmc_bb.dll (${featureList(res.features)})`;
  } catch {
    /* surfaced via store.error */
  } finally {
    stage.value = "";
  }
}

/** Whether Modkit's ledger records installing `key`, so there is something of its to remove. */
function recorded(key: "pmc_bb" | "dxwrapper"): boolean {
  return !!store.managedComponent(key)?.installedTag;
}

/** One line on what a removal did, naming every file it did not trash. */
function describeRemoval(label: string, r: ComponentRemoval): string {
  const parts = [`Removed ${label}: ${r.removed.length} file${r.removed.length === 1 ? "" : "s"} moved to Modkit's trash.`];
  if (r.kept_modified.length) parts.push(`Left in place because they were changed since Modkit installed them: ${r.kept_modified.join(", ")}.`);
  if (r.missing.length) parts.push(`Already gone: ${r.missing.join(", ")}.`);
  return parts.join(" ");
}

async function removeComponent(key: "pmc_bb" | "dxwrapper", label: string) {
  const ok = await ask(
    `Remove ${label} from the game folder? The files Modkit installed go to Modkit's trash. ` +
      "Nothing is put back in their place.",
    { title: `Remove ${label}`, kind: "warning" },
  );
  if (!ok) return;
  const msg = key === "pmc_bb" ? pmcMsg : dxMsg;
  msg.value = null;
  try {
    msg.value = describeRemoval(label, await store.removeManagedComponent(key));
  } catch {
    /* surfaced via store.error */
  }
}

async function setupDxwrapper() {
  stage.value = "Downloading dxwrapper + pmc_bb…";
  dxMsg.value = null;
  try {
    const res = await store.setupDxwrapper();
    dxMsg.value = res.ok
      ? `Installed dxwrapper ${res.version} → ${res.proxyPath}`
      : "dxwrapper install failed — see the error above.";
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

async function normalizeRegion() {
  stage.value = "Writing the matchmaking region… (approve the UAC prompt)";
  regionMsg.value = null;
  try {
    const res = await store.normalizeRegion();
    regionMsg.value = res.message;
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
        <h2 class="plate-title text-xl">Game Info</h2>
        <p class="text-sm text-zinc-500">
          What modkit detected about your install and the pmc_bb.dll ASI loader,
          plus any available updates.
        </p>
      </div>
      <button
        v-if="gameInfo"
        class="btn-outline shrink-0"
        :disabled="busy || checking"
        @click="refreshAll"
      >
        {{ checking ? "Checking…" : "Refresh & check updates" }}
      </button>
    </header>    <div
      v-if="!gameInfo"
      class="empty-plate mt-10"
    >
      Set your game folder in the bar above to see install details.
    </div>

    <template v-else>
      <ErrorBanner />

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
          <template v-if="gameInfo.launch">
            Launching
            <span class="font-mono text-xs">{{ gameInfo.launch.exe.path }}</span>
            ({{ gameInfo.launch.selection === "override" ? "pinned" : "chosen by modkit" }}).
            <template v-if="store.gameFullySetUp">
              {{
                store.setupPath === "dxwrapper"
                  ? "dxwrapper loads pmc_bb.dll; the exe is untouched."
                  : "The exe loads pmc_bb.dll itself."
              }}
            </template>
            <template v-else-if="store.setupPath === 'crack'">
              Stock SecuROM exe with no activation — crack it below (or in
              <RouterLink to="/setup" class="underline">Setup</RouterLink>).
            </template>
          </template>
          <template v-else>Nothing to launch: {{ gameInfo.launch_error }}</template>
        </p>
        <ul
          v-if="gameInfo.launch?.problems.length"
          class="mt-2 list-disc pl-5 text-sm text-amber-300/90"
        >
          <li v-for="p in gameInfo.launch.problems" :key="p">{{ p }}</li>
        </ul>
        <p
          v-if="store.vcRedistMissing"
          class="mt-2 text-sm font-medium text-amber-300"
        >
          ⚠ The 32-bit Visual C++ 2008 runtime is missing — the game won't launch
          (“binkw32.dll was not found”) until it's installed below.
        </p>
        <p
          v-if="store.regionNeedsNormalize"
          class="mt-2 text-sm font-medium text-amber-300"
        >
          ⚠ Matchmaking region doesn't match your selection — you won't see
          other players in multiplayer until you apply it below.
        </p>
      </div>

      <!-- Wine (macOS) / Proton (Linux), hidden on Windows. Outside the game-folder
          gate: the runtime is set up before the game can run, not after. -->
      <RuntimePanel class="mt-4" />

      <!-- Game details -->
      <section class="guilloche mt-4 rounded-xl border border-zinc-800 p-5">
        <h3 class="plate-title text-sm">Game</h3>
        <dl class="mt-3 space-y-2 text-sm">
          <div class="flex gap-3">
            <dt class="w-32 shrink-0 text-zinc-500">Install folder</dt>
            <dd class="min-w-0 break-all font-mono text-xs text-zinc-300">
              {{ gameInfo.root }}
            </dd>
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

        <!-- Which exe launches. Picking one pins it; modkit then stops switching
             anything automatically for the launch. -->
        <h4 class="mt-5 text-xs tracking-wider text-zinc-500 uppercase">Executable to launch</h4>
        <div class="mt-2 space-y-1.5 text-sm">
          <label class="flex items-start gap-2 text-zinc-300">
            <input
              type="radio"
              class="mt-1"
              :checked="pinnedExe === null && !gameInfo.launch_error"
              :disabled="busy"
              @change="pinExe(null)"
            />
            <span>Automatic — modkit picks the best identified exe</span>
          </label>
          <label
            v-for="e in gameInfo.exes"
            :key="e.path"
            class="flex items-start gap-2 text-zinc-300"
          >
            <input
              type="radio"
              class="mt-1"
              :checked="pinnedExe === e.path"
              :disabled="busy"
              @change="pinExe(e.path)"
            />
            <span class="min-w-0">
              <span class="font-mono text-xs break-all">{{ e.name }}</span>
              <span
                v-if="gameInfo.launch?.exe.path === e.path"
                class="ml-1 text-xs text-emerald-300"
                >← launches</span
              >
              <span class="block text-xs" :class="e.id ? 'text-zinc-500' : 'text-amber-300/90'">
                <template v-if="e.id">
                  <span :class="versionTone(e.version ?? '')">{{ e.version }}</span>
                  {{ e.variant }} — {{ e.label }}
                </template>
                <template v-else>unrecognised build</template>
                · {{ fmtBytes(e.size) }} · sha256 {{ e.sha256.slice(0, 12) }}…
              </span>
            </span>
          </label>
          <button
            class="text-xs text-zinc-500 underline hover:text-zinc-300"
            :disabled="busy"
            @click="browseExe"
          >
            Pin an exe from elsewhere…
          </button>
        </div>
      </section>

      <!-- pmc_bb.dll — shown on every setup path.
           It used to be hidden whenever dxwrapper was the loader, on the reasoning
           that the dxwrapper section installed it. But a licensed copy runs a
           pmc_bb.dll like everyone else, gets releases like everyone else, and had
           no control here to update it. -->
      <section class="guilloche mt-4 rounded-xl border border-zinc-800 p-5">
        <div class="flex items-start justify-between gap-4">
          <div class="min-w-0">
            <h3 class="plate-title text-sm">pmc_bb.dll</h3>
            <p class="mt-1 text-sm text-zinc-400">
              Published as six builds over three features — SecuROM spoof, ASI
              loader, logging. modkit picks the one your exe needs.
            </p>

            <dl class="mt-3 space-y-2 text-sm">
              <div class="flex items-center gap-3">
                <dt class="w-32 shrink-0 text-zinc-500">Status</dt>
                <dd>
                  <span
                    class="stamp"
                    :class="
                      gameInfo.has_pmc_bb
                        ? 'text-emerald-300'
                        : 'text-zinc-500'
                    "
                    >{{ gameInfo.has_pmc_bb ? "Installed ✓" : "Not installed" }}</span
                  >
                </dd>
              </div>
              <div v-if="gameInfo.has_pmc_bb" class="flex items-center gap-3">
                <dt class="w-32 shrink-0 text-zinc-500">Installed build</dt>
                <dd class="font-mono text-xs text-zinc-300">
                  {{ pmcBbAsset ?? "unknown (not installed by modkit)" }}
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
                <dd class="text-zinc-300">{{ pmcBbUpdate?.latest || "—" }}</dd>
              </div>
              <div v-if="recommended" class="flex items-start gap-3">
                <dt class="w-32 shrink-0 text-zinc-500">Recommended</dt>
                <dd class="min-w-0 text-zinc-300">
                  <span class="font-mono text-xs">{{ recommended.asset }}</span>
                  <span class="ml-1 text-xs text-zinc-500">
                    ({{ featureList(recommended.features) }})
                  </span>
                  <p class="mt-1 text-xs text-zinc-500">{{ recommended.reason }}</p>
                </dd>
              </div>
              <div v-if="wanted?.overridden" class="flex items-center gap-3">
                <dt class="w-32 shrink-0 text-zinc-500">Pinned</dt>
                <dd class="font-mono text-xs text-amber-300">{{ wanted.asset }}</dd>
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
              v-else-if="pmcBbPending"
              class="mt-3 text-sm text-zinc-400"
            >
              {{ pmcBbPending }}
            </p>
            <p
              v-else-if="gameInfo.has_pmc_bb && pmcBbVersion && pmcBbUpdate?.latest"
              class="mt-3 text-sm text-emerald-300/80"
            >
              Up to date ✓
            </p>
            <p
              v-else-if="gameInfo.has_pmc_bb && !pmcBbVersion"
              class="mt-3 text-xs text-zinc-500"
            >
              modkit didn't install this copy, so it can't tell which build or
              version it is. Reinstall to start tracking it.
            </p>

            <p
              v-if="variantMismatch"
              class="mt-2 flex items-center gap-1.5 text-xs text-amber-300"
            >
              <span class="h-1.5 w-1.5 rounded-full bg-amber-400" />
              The installed build isn't
              {{ wanted?.overridden ? "the one you pinned" : "the one this exe calls for" }} —
              reinstall to switch to {{ wanted?.asset }}.
            </p>
            <p v-if="pmcBbModified" class="mt-2 text-xs text-amber-300">
              This file has changed since modkit installed it — something replaced
              it by hand.
            </p>

            <!-- Advanced: force a specific build. -->
            <div v-if="variants.length" class="mt-3">
              <button
                class="text-xs text-zinc-500 underline hover:text-zinc-300"
                @click="showVariants = !showVariants"
              >
                {{ showVariants ? "Hide" : "Choose a build manually" }}
              </button>
              <div v-if="showVariants" class="mt-2 space-y-1">
                <label class="flex items-start gap-2 text-xs text-zinc-400">
                  <input
                    v-model="chosenVariant"
                    type="radio"
                    value=""
                    class="mt-0.5"
                  />
                  <span>Let modkit choose (recommended)</span>
                </label>
                <label
                  v-for="v in variants"
                  :key="v.asset"
                  class="flex items-start gap-2 text-xs text-zinc-400"
                >
                  <input
                    v-model="chosenVariant"
                    type="radio"
                    :value="v.asset"
                    class="mt-0.5"
                  />
                  <span>
                    <span class="font-mono">{{ v.asset }}</span> — {{ v.blurb }}
                  </span>
                </label>
              </div>
            </div>

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
              pmcBbUpdate?.available || variantMismatch
                ? 'bg-amber-500 text-zinc-900 hover:bg-amber-400'
                : 'bg-emerald-600 text-white hover:bg-emerald-500'
            "
            :disabled="busy"
            @click="installPmcBb"
          >
            {{
              pmcBbUpdate?.available
                ? "Update"
                : variantMismatch
                  ? "Switch build"
                  : gameInfo.has_pmc_bb
                    ? "Reinstall"
                    : "Install"
            }}
          </button>
          <button
            v-if="recorded('pmc_bb')"
            class="shrink-0 rounded-lg px-3 py-2 text-sm text-zinc-400 hover:bg-red-500/10 hover:text-red-400 disabled:opacity-50"
            :disabled="busy"
            title="Move the pmc_bb.dll Modkit installed to Modkit's trash"
            @click="removeComponent('pmc_bb', 'pmc_bb')"
          >
            Remove
          </button>
        </div>
      </section>

      <!-- Exe: official v1.1 update and the crack. Shown on every setup path and
           for every exe — the only gate is each panel's own "already done". -->
      <ExeUpdatePanel class="mt-4" />
      <CrackPanel class="mt-4" />

      <!-- dxwrapper -->
      <section
        v-if="store.setupPath === 'dxwrapper' || gameInfo.has_dxwrapper"
        class="guilloche mt-4 rounded-xl border border-zinc-800 p-5"
      >
        <div class="flex items-start justify-between gap-4">
          <div class="min-w-0">
            <h3 class="plate-title text-sm">dxwrapper</h3>
            <p class="mt-1 text-sm text-zinc-400">
              DirectX wrapper (elishacloud/dxwrapper) — modkit installs and keeps it up to date.
            </p>

            <dl class="mt-3 space-y-2 text-sm">
              <div class="flex items-center gap-3">
                <dt class="w-32 shrink-0 text-zinc-500">Status</dt>
                <dd>
                  <span
                    class="stamp"
                    :class="gameInfo.has_dxwrapper ? 'text-emerald-300' : 'text-zinc-500'"
                    >{{ gameInfo.has_dxwrapper ? "Installed ✓" : "Not installed" }}</span
                  >
                </dd>
              </div>
              <div class="flex items-center gap-3">
                <dt class="w-32 shrink-0 text-zinc-500">Installed version</dt>
                <dd class="text-zinc-300">
                  {{ gameInfo.has_dxwrapper ? (dxwrapperVersion ?? "unknown") : "—" }}
                </dd>
              </div>
              <div class="flex items-center gap-3">
                <dt class="w-32 shrink-0 text-zinc-500">Latest release</dt>
                <dd class="text-zinc-300">{{ dxwrapperUpdate?.latest ?? "—" }}</dd>
              </div>
            </dl>

            <p
              v-if="dxwrapperUpdate?.available"
              class="mt-3 flex items-center gap-1.5 text-sm font-medium text-amber-300"
            >
              <span class="h-1.5 w-1.5 rounded-full bg-amber-400" />
              Update available → {{ dxwrapperUpdate.latest }}
            </p>

            <p
              v-if="dxMsg"
              class="mt-3 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-300"
            >
              {{ dxMsg }}
            </p>
            <p v-if="stage" class="mt-2 text-xs text-zinc-500">{{ stage }}</p>
          </div>

          <button
            class="shrink-0 rounded-lg px-3 py-2 text-sm font-medium disabled:opacity-50"
            :class="
              dxwrapperUpdate?.available
                ? 'bg-amber-500 text-zinc-900 hover:bg-amber-400'
                : 'bg-emerald-600 text-white hover:bg-emerald-500'
            "
            :disabled="busy"
            @click="setupDxwrapper"
          >
            {{
              dxwrapperUpdate?.available
                ? "Update"
                : gameInfo.has_dxwrapper
                  ? "Reinstall"
                  : "Install"
            }}
          </button>
          <button
            v-if="recorded('dxwrapper')"
            class="shrink-0 rounded-lg px-3 py-2 text-sm text-zinc-400 hover:bg-red-500/10 hover:text-red-400 disabled:opacity-50"
            :disabled="busy"
            title="Move the dxwrapper files Modkit installed to Modkit's trash"
            @click="removeComponent('dxwrapper', 'dxwrapper')"
          >
            Remove
          </button>
        </div>
      </section>

      <!-- Matchmaking region (registry) -->
      <section
        v-if="region?.applicable"
        class="mt-4 rounded-xl border p-5"
        :class="
          region.normalized
            ? 'border-zinc-800'
            : 'border-amber-500/40 bg-amber-500/5'
        "
      >
        <div class="flex items-start justify-between gap-4">
          <div class="min-w-0">
            <h3 class="plate-title text-sm">Matchmaking region</h3>
            <p class="mt-1 text-sm text-zinc-400">
              The game keys its multiplayer version off the
              <code class="text-zinc-300">Region</code> registry value, so you
              only see players whose installs share yours. Pick the region your
              group uses — the community default is North America — and apply
              it (this also writes your install path).
            </p>

            <dl class="mt-3 space-y-2 text-sm">
              <div class="flex items-center gap-3">
                <dt class="w-32 shrink-0 text-zinc-500">Status</dt>
                <dd>
                  <span
                    class="stamp"
                    :class="
                      region.normalized
                        ? 'text-emerald-300'
                        : 'text-amber-300'
                    "
                    >{{ region.normalized ? "Applied ✓" : "Not applied" }}</span
                  >
                </dd>
              </div>
              <div class="flex items-center gap-3">
                <dt class="w-32 shrink-0 text-zinc-500">Current region</dt>
                <dd class="font-mono text-xs text-zinc-300">
                  {{ region.currentRegion ?? "— (no key)" }}
                </dd>
              </div>
              <div class="flex items-center gap-3">
                <dt class="w-32 shrink-0 text-zinc-500">Your region</dt>
                <dd>
                  <select
                    class="field px-2 py-1 text-xs disabled:opacity-50"
                    :value="region.expectedRegion"
                    :disabled="busy"
                    @change="onRegionSelect"
                  >
                    <option
                      v-for="r in region.knownRegions"
                      :key="r"
                      :value="r"
                    >
                      {{ regionLabel(r) }} ({{ r }})
                    </option>
                  </select>
                </dd>
              </div>
            </dl>

            <p class="mt-3 text-sm text-zinc-400">{{ region.detail }}</p>
            <p
              v-if="!region.normalized"
              class="mt-2 text-xs text-amber-300/90"
            >
              Writing under HKLM needs admin — you'll see a Windows UAC prompt.
            </p>

            <p
              v-if="regionMsg"
              class="mt-3 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-300"
            >
              {{ regionMsg }}
            </p>
          </div>

          <button
            class="shrink-0 rounded-lg px-3 py-2 text-sm font-medium disabled:opacity-50"
            :class="
              region.normalized
                ? 'border border-zinc-700 text-zinc-300 hover:bg-zinc-800'
                : 'bg-amber-500 text-zinc-900 hover:bg-amber-400'
            "
            :disabled="busy"
            @click="normalizeRegion"
          >
            {{ region.normalized ? "Re-write" : "Apply region" }}
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
            <h3 class="plate-title text-sm">
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
                    class="stamp"
                    :class="
                      vcRedist.installed
                        ? 'text-emerald-300'
                        : 'text-amber-300'
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
