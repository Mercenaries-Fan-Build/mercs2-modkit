<script setup lang="ts">
import { ref } from "vue";
import { storeToRefs } from "pinia";
import { useProjectStore } from "../stores/project";
import type { CrackResult } from "../types";

// EA's official v1.0 → v1.1 update, no crack (apply_crack --update-only). The
// backend backs the original up to BACKUP/ and swaps the update in only when the
// tool succeeds. Gated only on being done already (the exe is v1.1) — never on
// signing or variant; apply_crack reports what it makes of the input.
withDefaults(defineProps<{ title?: string }>(), {
  title: "Update to v1.1 (official patch)",
});

const store = useProjectStore();
const { gameInfo, busy } = storeToRefs(store);

const stage = ref("");
const result = ref<CrackResult | null>(null);

async function runUpdate() {
  stage.value = "Updating exe to v1.1 (official patch, no crack)…";
  result.value = null;
  try {
    result.value = await store.updateGame();
  } catch {
    /* surfaced via store.error */
  } finally {
    stage.value = "";
  }
}
</script>

<template>
  <section class="guilloche rounded-xl border border-zinc-800 p-5">
    <div class="flex items-start justify-between gap-4">
      <div>
        <h3 class="plate-title text-sm">{{ title }}</h3>
        <p class="mt-1 text-sm text-zinc-400">
          Applies EA's official v1.0 → v1.1 update — <span class="text-zinc-300">not a crack</span>.
          The exe stays SecuROM-protected and your activation carries over. Your original is
          backed up to <span class="font-mono text-xs">BACKUP/</span>.
        </p>
      </div>
      <span v-if="gameInfo?.version === 'v1.1'" class="stamp shrink-0 text-emerald-300">
        Already v1.1 ✓
      </span>
      <button
        v-else
        class="shrink-0 rounded-lg bg-zinc-700 px-3 py-2 text-sm font-medium text-white hover:bg-zinc-600 disabled:opacity-50"
        :disabled="busy"
        @click="runUpdate"
      >
        Update
      </button>
    </div>
    <p v-if="stage" class="mt-2 text-xs text-zinc-500">{{ stage }}</p>
    <p
      v-if="result"
      class="mt-3 text-sm"
      :class="result.ok ? 'text-emerald-400' : 'text-red-400'"
    >
      {{ result.ok ? "Updated to v1.1 (DRM intact)" : "Update failed" }} —
      {{ result.stdout || result.stderr }}
    </p>
  </section>
</template>
