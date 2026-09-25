<script setup lang="ts">
/**
 * Confirm a Shipment removal, showing the whole chain first: the Shipment the
 * player chose, every Shipment the cascade takes with the requirement that pulls it in, and the
 * dependencies nothing will require any more. One confirmation removes exactly that list;
 * Cancel removes nothing.
 */
import { computed } from "vue";
import { storeToRefs } from "pinia";
import { useProjectStore } from "../stores/project";
import ConfirmDialog from "./ConfirmDialog.vue";
import type { PulledBy } from "../types";

const store = useProjectStore();
const { pendingShipmentRemoval: plan } = storeToRefs(store);

const total = computed(() =>
  plan.value ? 1 + plan.value.cascade.length + plan.value.orphans.length : 0,
);

function requirement(p: PulledBy): string {
  if (p.kind === "capability") return `requires the capability “${p.target}”, whose last provider goes`;
  return p.range ? `requires ${p.target} ${p.range}` : `requires ${p.target}`;
}
</script>

<template>
  <ConfirmDialog
    :open="!!plan"
    :title="`Remove ${plan?.removed.name ?? ''}?`"
    :confirm-label="total > 1 ? `Remove ${total} Shipments` : 'Remove'"
    danger
    @confirm="store.confirmShipmentRemoval()"
    @cancel="store.cancelShipmentRemoval()"
  >
    <template v-if="plan">
      <p>
        <strong class="text-zinc-200">{{ plan.removed.name }}</strong>
        will be removed from the Library. Its staged files go to Modkit's trash.
      </p>
      <div
        v-if="plan.cascade.length"
        class="mt-3 rounded-lg border border-amber-600/30 bg-amber-500/10 px-3 py-2 text-amber-200"
      >
        <p class="font-medium">These need it, so they are removed too:</p>
        <ul class="mt-1 list-inside list-disc">
          <li v-for="c in plan.cascade" :key="c.shipment.id">
            {{ c.shipment.name }}
            <span class="text-amber-300/80">— {{ requirement(c.pulled_by) }}</span>
          </li>
        </ul>
      </div>
      <div
        v-if="plan.orphans.length"
        class="mt-3 rounded-lg border border-zinc-700 bg-zinc-800/60 px-3 py-2 text-zinc-300"
      >
        <p class="font-medium">These were installed only as dependencies, and nothing will need them:</p>
        <ul class="mt-1 list-inside list-disc">
          <li v-for="o in plan.orphans" :key="o.id">{{ o.name }}</li>
        </ul>
      </div>
    </template>
  </ConfirmDialog>
</template>
