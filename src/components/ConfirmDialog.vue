<script setup lang="ts">
import { Dialog, DialogPanel, DialogTitle } from "@headlessui/vue";

withDefaults(
  defineProps<{
    open: boolean;
    title: string;
    confirmLabel?: string;
    cancelLabel?: string;
    danger?: boolean;
  }>(),
  { confirmLabel: "Confirm", cancelLabel: "Cancel", danger: false }
);

const emit = defineEmits<{ confirm: []; cancel: [] }>();
</script>

<template>
  <Dialog :open="open" class="relative z-50" @close="emit('cancel')">
    <div class="fixed inset-0 bg-black/60" aria-hidden="true" />
    <div class="fixed inset-0 flex items-center justify-center p-4">
      <DialogPanel
        class="w-full max-w-md rounded-xl border border-zinc-800 bg-zinc-900 p-5 shadow-2xl"
      >
        <DialogTitle class="text-base font-semibold text-zinc-100">
          {{ title }}
        </DialogTitle>
        <div class="mt-2 text-sm text-zinc-400">
          <slot />
        </div>
        <div class="mt-5 flex justify-end gap-2">
          <button
            class="rounded-lg border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 hover:bg-zinc-800"
            @click="emit('cancel')"
          >
            {{ cancelLabel }}
          </button>
          <button
            class="rounded-lg px-3 py-1.5 text-sm font-medium text-white"
            :class="
              danger
                ? 'bg-red-600 hover:bg-red-500'
                : 'bg-emerald-600 hover:bg-emerald-500'
            "
            @click="emit('confirm')"
          >
            {{ confirmLabel }}
          </button>
        </div>
      </DialogPanel>
    </div>
  </Dialog>
</template>
