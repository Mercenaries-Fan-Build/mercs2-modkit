<script setup lang="ts">
defineProps<{ count: number }>();
</script>

<template>
  <span
    v-if="count > 0"
    class="stamp text-amber-300"
  >
    <span class="h-1.5 w-1.5 rounded-full bg-amber-400" />
    {{ count }} conflict{{ count === 1 ? "" : "s" }}
  </span>
  <span
    v-else
    class="stamp text-emerald-300"
  >
    <span class="h-1.5 w-1.5 rounded-full bg-emerald-400" />
    no conflicts
  </span>
</template>
