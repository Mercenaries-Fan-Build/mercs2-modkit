<script setup lang="ts">
/**
 * What mercs.ink's community incompatibility list said about the last build: the unconfirmed
 * reports that apply to its Shipments, and a line when the list was a cached copy or was never
 * downloaded. A confirmed report refuses the build instead, so it never shows here. Every
 * sentence comes from the backend; this only lays them out.
 */
import { computed } from "vue";
import { storeToRefs } from "pinia";
import { useProjectStore } from "../stores/project";

const store = useProjectStore();
const { incompatibilityCheck } = storeToRefs(store);

const list = computed(() => incompatibilityCheck.value?.list ?? null);
const notices = computed(() => incompatibilityCheck.value?.notices ?? []);
const visible = computed(
  () => list.value !== null && (notices.value.length > 0 || list.value.state !== "current"),
);

/** When the cached list was downloaded, in the viewer's local time. */
const fetchedAtLocal = computed(() =>
  list.value?.state === "cached" ? new Date(list.value.fetched_at * 1000).toLocaleString() : null,
);
</script>

<template>
  <div
    v-if="visible && list"
    class="flex flex-col gap-1.5 border-b border-amber-600/30 bg-amber-500/10 px-4 py-3 text-sm text-amber-200"
  >
    <p v-for="(n, i) in notices" :key="i" class="min-w-0">
      {{ n.message }}
    </p>
    <p v-if="list.state === 'cached'" class="min-w-0 text-amber-300/80">
      {{ list.message }} (Downloaded {{ fetchedAtLocal }}.)
    </p>
    <p v-else-if="list.state === 'never_fetched'" class="min-w-0 text-amber-300/80">
      {{ list.message }}
    </p>
  </div>
</template>
