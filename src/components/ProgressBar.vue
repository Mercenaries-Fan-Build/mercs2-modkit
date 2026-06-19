<script setup lang="ts">
defineProps<{ indeterminate?: boolean; value?: number; label?: string }>();
</script>

<template>
  <div class="space-y-1">
    <p v-if="label" class="text-xs text-zinc-400">{{ label }}</p>
    <div class="h-2 w-full overflow-hidden rounded-full bg-zinc-800">
      <div
        v-if="indeterminate"
        class="h-full w-1/3 animate-pulse rounded-full bg-emerald-500"
      />
      <div
        v-else
        class="h-full rounded-full bg-emerald-500 transition-all"
        :style="{ width: `${Math.min(100, Math.max(0, value ?? 0))}%` }"
      />
    </div>
  </div>
</template>
