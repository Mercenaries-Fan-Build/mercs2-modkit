<script setup lang="ts">
defineProps<{ count: number }>();
</script>

<template>
  <span
    v-if="count > 0"
    class="inline-flex items-center gap-1 rounded-full bg-amber-500/15 px-2 py-0.5 text-xs font-medium text-amber-300"
  >
    <span class="h-1.5 w-1.5 rounded-full bg-amber-400" />
    {{ count }} conflict{{ count === 1 ? "" : "s" }}
  </span>
  <span
    v-else
    class="inline-flex items-center gap-1 rounded-full bg-emerald-500/15 px-2 py-0.5 text-xs font-medium text-emerald-300"
  >
    <span class="h-1.5 w-1.5 rounded-full bg-emerald-400" />
    no conflicts
  </span>
</template>
