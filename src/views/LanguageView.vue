<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { storeToRefs } from "pinia";
import { useProjectStore } from "../stores/project";
import type {
  LanguageStatus,
  LanguagePresence,
  SetLanguageResult,
  AddedLanguage,
} from "../types";
import ConfirmDialog from "../components/ConfirmDialog.vue";
import Spinner from "../components/Spinner.vue";

const store = useProjectStore();
const { gameInfo, busy } = storeToRefs(store);

const status = ref<LanguageStatus | null>(null);
const scanning = ref(false);
const opError = ref<string | null>(null);
const result = ref<SetLanguageResult | null>(null);

// The language awaiting confirmation in the dialog.
const pending = ref<LanguagePresence | null>(null);

// Languages that actually have content on disk.
const present = computed(
  () => status.value?.languages.filter((l) => l.wadPresent || l.pwsPresent) ?? []
);

function langBytes(l: LanguagePresence): number {
  return l.wadSize + l.pwsSize;
}

function fmtBytes(n: number): string {
  if (!n) return "—";
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

async function scan() {
  if (!gameInfo.value) return;
  scanning.value = true;
  opError.value = null;
  result.value = null;
  try {
    status.value = await store.scanLanguages();
  } catch (e) {
    opError.value = String(e);
  } finally {
    scanning.value = false;
  }
}

function askKeep(l: LanguagePresence) {
  pending.value = l;
}

async function confirmKeep() {
  const l = pending.value;
  pending.value = null;
  if (!l) return;
  opError.value = null;
  result.value = null;
  try {
    result.value = await store.setLanguage(l.language);
    await scan();
  } catch (e) {
    opError.value = String(e);
  }
}

// --- Added (novel) languages ---
const added = computed<AddedLanguage[]>(() => status.value?.added ?? []);
const selector = computed(() => status.value?.selector ?? null);
const addedMsg = ref<string | null>(null);

async function useAdded(l: AddedLanguage) {
  opError.value = null;
  addedMsg.value = null;
  try {
    await store.setAddedLanguage(l.name);
    addedMsg.value = `Selected ${l.display}. Relaunch the game to switch into it.`;
    await scan();
  } catch (e) {
    opError.value = String(e);
  }
}

async function clearAdded() {
  opError.value = null;
  addedMsg.value = null;
  try {
    await store.clearAddedLanguage();
    addedMsg.value = "Cleared — the game will use its normal boot language.";
    await scan();
  } catch (e) {
    opError.value = String(e);
  }
}

// Scan whenever the game folder is set or changes.
watch(
  () => gameInfo.value?.root,
  (root) => {
    if (root) void scan();
    else status.value = null;
  },
  { immediate: true }
);
</script>

<template>
  <div class="mx-auto max-w-3xl px-8 py-6">
    <header class="flex items-start justify-between gap-4">
      <div>
        <h2 class="plate-title text-xl">Language</h2>
        <p class="text-sm text-zinc-500">
          Each language ships its own <code class="text-zinc-400">.wad</code> +
          voice-over <code class="text-zinc-400">.pws</code>. Keep the one you
          play in; the rest are moved to the recoverable trash to reclaim space.
        </p>
      </div>
      <button
        v-if="gameInfo"
        class="btn-outline shrink-0"
        :disabled="scanning || busy"
        @click="scan"
      >
        {{ scanning ? "Scanning…" : "Rescan" }}
      </button>
    </header>

    <div
      v-if="!gameInfo"
      class="empty-plate mt-10"
    >
      Set your game folder in the bar above to manage language content.
    </div>

    <template v-else>
      <p
        v-if="opError"
        class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
      >
        {{ opError }}
      </p>

      <p
        v-if="result"
        class="mt-4 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-4 py-3 text-sm text-emerald-300"
      >
        Kept <strong>{{ result.kept }}</strong
        >.
        <template v-if="result.removed.length">
          Moved {{ result.removed.length }} file{{
            result.removed.length === 1 ? "" : "s"
          }}
          ({{ fmtBytes(result.freedBytes) }}) to the trash.
        </template>
        <template v-else>No other language content was present.</template>
      </p>

      <div
        v-if="scanning && !status"
        class="mt-6 flex items-center gap-2 text-sm text-zinc-500"
      >
        <Spinner /> Scanning install…
      </div>

      <template v-if="status">
        <div
          v-if="present.length === 0"
          class="empty-plate mt-6"
        >
          No recognized language content found in
          <code class="text-zinc-400">{{ status.dataDir ?? "the data folder" }}</code
          >.
        </div>

        <template v-else>
          <p class="mt-6 text-sm text-zinc-400">
            {{ present.length }} language{{ present.length === 1 ? "" : "s" }}
            present. Choose one to keep:
          </p>

          <ul class="mt-3 space-y-2">
            <li
              v-for="l in present"
              :key="l.language"
              class="guilloche flex items-center justify-between gap-4 rounded-xl border border-zinc-800 p-4"
            >
              <div class="min-w-0">
                <p class="font-medium text-zinc-100">
                  {{ l.language }}
                  <span class="ml-2 text-xs font-normal text-zinc-500">
                    {{ l.locales.join(", ") }}
                  </span>
                </p>
                <p class="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-xs">
                  <span :class="l.wadPresent ? 'text-zinc-400' : 'text-zinc-600'">
                    {{ l.wadName }}:
                    {{ l.wadPresent ? fmtBytes(l.wadSize) : "missing" }}
                  </span>
                  <span :class="l.pwsPresent ? 'text-zinc-400' : 'text-zinc-600'">
                    {{ l.pwsName }}:
                    {{ l.pwsPresent ? fmtBytes(l.pwsSize) : "missing" }}
                  </span>
                </p>
              </div>
              <div class="flex shrink-0 items-center gap-3">
                <span class="text-xs text-zinc-500">
                  {{ fmtBytes(langBytes(l)) }}
                </span>
                <button
                  class="btn-plate"
                  :disabled="busy || present.length === 1"
                  :title="
                    present.length === 1
                      ? 'Only one language present — nothing to remove'
                      : `Keep ${l.language}, trash the rest`
                  "
                  @click="askKeep(l)"
                >
                  Keep this
                </button>
              </div>
            </li>
          </ul>

          <p v-if="present.length === 1" class="mt-3 text-xs text-zinc-500">
            Only one language is installed — nothing to remove.
          </p>
        </template>
      </template>

      <!-- Added (novel) languages — installed by a mod, switched via the selector plugin -->
      <template v-if="status && (added.length || selector?.pluginInstalled)">
        <div class="mt-8 border-t border-zinc-800 pt-6">
          <h3 class="plate-title text-base">Added languages</h3>
          <p class="mt-1 text-sm text-zinc-500">
            Languages a mod installed that the game never shipped. The game has no
            in-game language picker, so a selector plugin switches into one at launch.
          </p>

          <p
            v-if="addedMsg"
            class="mt-4 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-4 py-3 text-sm text-emerald-300"
          >
            {{ addedMsg }}
          </p>

          <p
            v-if="selector && !selector.pluginInstalled"
            class="mt-4 rounded-lg border border-amber-500/30 bg-amber-500/10 px-4 py-3 text-sm text-amber-300"
          >
            The language-selector plugin isn't installed, so these can't be
            switched on. Install the language mod to enable selection.
          </p>

          <div
            v-if="added.length === 0"
            class="empty-plate mt-4"
          >
            No added languages installed.
          </div>

          <ul
            v-else
            class="mt-3 space-y-2"
          >
            <li
              v-for="l in added"
              :key="l.name"
              class="guilloche flex items-center justify-between gap-4 rounded-xl border p-4"
              :class="l.active ? 'border-emerald-500/40' : 'border-zinc-800'"
            >
              <div class="min-w-0">
                <p class="font-medium text-zinc-100">
                  {{ l.display }}
                  <span
                    v-if="l.active"
                    class="ml-2 rounded bg-emerald-500/15 px-1.5 py-0.5 text-xs font-normal text-emerald-300"
                  >
                    Active
                  </span>
                </p>
                <p class="mt-1 text-xs text-zinc-500">
                  {{ l.wadName }}: {{ fmtBytes(l.wadSize) }}
                </p>
              </div>
              <div class="flex shrink-0 items-center gap-3">
                <button
                  v-if="!l.active"
                  class="btn-plate"
                  :disabled="busy || !selector?.pluginInstalled"
                  :title="
                    selector?.pluginInstalled
                      ? `Switch the game into ${l.display} at next launch`
                      : 'The selector plugin is not installed'
                  "
                  @click="useAdded(l)"
                >
                  Use this language
                </button>
                <button
                  v-else
                  class="btn-outline"
                  :disabled="busy"
                  @click="clearAdded"
                >
                  Use default
                </button>
              </div>
            </li>
          </ul>
        </div>
      </template>
    </template>

    <ConfirmDialog
      :open="!!pending"
      :title="`Keep ${pending?.language} only?`"
      :confirm-label="`Keep ${pending?.language}`"
      cancel-label="Cancel"
      danger
      @cancel="pending = null"
      @confirm="confirmKeep"
    >
      Every other language's <code class="text-zinc-300">.wad</code> and
      <code class="text-zinc-300">.pws</code> will be moved to the modkit trash
      (recoverable). The game will use {{ pending?.language }} content only.
    </ConfirmDialog>
  </div>
</template>
