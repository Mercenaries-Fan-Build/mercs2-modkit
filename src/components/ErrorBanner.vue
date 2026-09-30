<script setup lang="ts">
import { storeToRefs } from "pinia";
import { useProjectStore } from "../stores/project";

const store = useProjectStore();
const { error } = storeToRefs(store);

function dismiss() {
  store.error = null;
}
</script>

<template>
  <div
    v-if="error"
    role="alert"
    class="mt-4 flex items-start gap-3 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
  >
    <span class="flex-1 whitespace-pre-wrap break-words">{{ error }}</span>
    <button
      type="button"
      aria-label="Dismiss error"
      class="shrink-0 rounded p-0.5 leading-none text-red-300/70 hover:bg-red-500/20 hover:text-red-200 focus:outline-none focus:ring-1 focus:ring-red-400/50"
      @click="dismiss"
    >
      ✕
    </button>
  </div>
</template>
