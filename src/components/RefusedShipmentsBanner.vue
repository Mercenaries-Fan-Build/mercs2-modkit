<script setup lang="ts">
/**
 * Shown while the saved Shipment rows are refused (a library saved before dependency tracking,
 * or rows the backend cannot read). The rows stay in storage untouched; the only way to remove
 * them is this explicit action (user, 2026-09-24).
 */
import { ref } from "vue";
import { storeToRefs } from "pinia";
import { useProjectStore } from "../stores/project";
import ConfirmDialog from "./ConfirmDialog.vue";

const store = useProjectStore();
const { refusedShipments } = storeToRefs(store);
const confirming = ref(false);

function discard() {
  confirming.value = false;
  store.discardRefusedShipments();
}
</script>

<template>
  <div
    v-if="refusedShipments"
    class="flex items-start gap-3 border-b border-amber-600/30 bg-amber-500/10 px-4 py-3 text-sm text-amber-200"
  >
    <p class="min-w-0 flex-1 whitespace-pre-line">
      {{ refusedShipments.message }}
      <span class="block text-amber-300/80">
        {{ refusedShipments.count }} saved Shipment row{{ refusedShipments.count === 1 ? "" : "s" }}
        kept untouched. No Shipment can be added or removed until you discard them.
      </span>
    </p>
    <button
      class="shrink-0 rounded-lg bg-red-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-red-500"
      @click="confirming = true"
    >
      Discard old Shipments
    </button>
    <ConfirmDialog
      :open="confirming"
      title="Discard the old Shipments?"
      confirm-label="Discard"
      danger
      @confirm="discard"
      @cancel="confirming = false"
    >
      <p>
        This removes the {{ refusedShipments.count }} saved Shipment row{{
          refusedShipments.count === 1 ? "" : "s"
        }} from the Library so it can be rebuilt. Install them again afterwards.
      </p>
    </ConfirmDialog>
  </div>
</template>
