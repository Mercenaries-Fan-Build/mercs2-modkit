<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { storeToRefs } from "pinia";
import { useProjectStore } from "../stores/project";
import type {
  LanguageStatus,
  LanguagePresence,
  SetLanguageResult,
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
