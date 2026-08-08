<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { storeToRefs } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useProjectStore } from "../stores/project";
import type { PmcBbChoice, PmcBbVariant } from "../types";

const store = useProjectStore();
const {
  gameInfo,
  busy,
  error,
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

// Which pmc_bb build modkit would install here, and why. Resolved from the exe's
// identity rather than the setup path — see managed::pmc_bb. Shown before the user
// commits, so "why this one" never needs to be guessed at.
const recommended = ref<PmcBbChoice | null>(null);
const variants = ref<PmcBbVariant[]>([]);
const showVariants = ref(false);
const chosenVariant = ref("");

/** The installed build differs from what this install should have. */
const variantMismatch = computed(
  () =>
    !!pmcBbAsset.value &&
    !!recommended.value &&
    pmcBbAsset.value !== recommended.value.asset,
);

async function loadPmcBbChoice() {
  if (!gameInfo.value) {
    recommended.value = null;
    return;
  }
  try {
    recommended.value = await invoke<PmcBbChoice>("resolve_pmc_bb", {
      gameRoot: gameInfo.value.root,
      variant: null,
    });
  } catch {
    recommended.value = null;
  }
  if (!variants.value.length) {
    try {
      variants.value = await invoke<PmcBbVariant[]>("pmc_bb_variants");
    } catch {
      /* leave the advanced picker empty */
    }
  }
}

watch(() => gameInfo.value?.root, loadPmcBbChoice, { immediate: true });

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
      loadPmcBbChoice(),
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
    const res = await store.installPmcBb(chosenVariant.value || undefined);
    pmcMsg.value = `Installed ${res.asset} ${res.version} as pmc_bb.dll (${featureList(res.features)})`;
    await loadPmcBbChoice();
  } catch {
    /* surfaced via store.error */
  } finally {
    stage.value = "";
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
    </header>

    <div
      v-if="!gameInfo"
      class="empty-plate mt-10"
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
            <template v-if="store.setupPath === 'licensed'">
              Licensed — dxwrapper + pmc_bb.dll installed, exe untouched.
              Launching
              <span class="font-mono text-xs">{{ gameInfo.launch_exe_path }}</span
              >.
            </template>
            <template v-else-if="store.setupPath === 'drm_free'">
              DRM-free exe + pmc_bb.dll installed — no crack or wrapper needed.
              Launching
              <span class="font-mono text-xs">{{ gameInfo.launch_exe_path }}</span
              >.
            </template>
            <template v-else>
              v1.1, cracked, and pmc_bb.dll installed — launching
              <span class="font-mono text-xs">{{ store.crackedBuild?.name }}</span
              >.
            </template>
          </template>
          <template v-else>
            <template v-if="store.setupPath === 'licensed'">
              Licensed copy — install dxwrapper + pmc_bb.dll below (or in
              <RouterLink to="/setup" class="underline">Setup</RouterLink>). Your
              exe is never modified.
            </template>
            <template v-else-if="store.setupPath === 'drm_free'">
              DRM-free exe — just install pmc_bb.dll below (or in
              <RouterLink to="/setup" class="underline">Setup</RouterLink>).
            </template>
            <template v-else>
              Needs v1.1 + cracked exe + pmc_bb.dll. Finish in
              <RouterLink to="/setup" class="underline">Setup</RouterLink>.
            </template>
          </template>
        </p>
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

      <!-- Game details -->
      <section class="guilloche mt-4 rounded-xl border border-zinc-800 p-5">
        <h3 class="plate-title text-sm">Game</h3>
        <dl class="mt-3 space-y-2 text-sm">
          <div class="flex items-center gap-3">
            <dt class="w-32 shrink-0 text-zinc-500">Version</dt>
            <dd>
              <span
                class="stamp"
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
          <div v-if="gameInfo.cracked_exe" class="flex gap-3">
            <dt class="w-32 shrink-0 text-zinc-500">Cracked exe</dt>
            <dd class="min-w-0 break-all font-mono text-xs text-zinc-300">
              {{ gameInfo.cracked_exe.path }}
              <span class="text-zinc-500">
                — {{ gameInfo.cracked_exe.version }}
                ({{ gameInfo.cracked_exe.variant }}),
                {{ fmtBytes(gameInfo.cracked_exe.size) }}</span
              >
            </dd>
          </div>
          <div class="flex gap-3">
            <dt class="w-32 shrink-0 text-zinc-500">Launches</dt>
            <dd class="min-w-0 break-all font-mono text-xs text-zinc-300">
              {{ gameInfo.launch_exe_path }}
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
              The installed build isn't the one this exe calls for — reinstall to
              switch to {{ recommended?.asset }}.
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
        </div>
      </section>

      <!-- dxwrapper -->
      <section
        v-if="store.setupPath === 'licensed' || gameInfo.has_dxwrapper"
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
