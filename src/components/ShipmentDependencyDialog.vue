<script setup lang="ts">
/**
 * Offer to add what the convoy's unmet Shipment requirements need from mercs.ink. Add installs
 * exactly the listed releases; Not now installs nothing. When mercs.ink can't meet them, the
 * reason is shown and nothing is offered.
 */
import { storeToRefs } from "pinia";
import { useProjectStore } from "../stores/project";
import ConfirmDialog from "./ConfirmDialog.vue";
import type { UnmetRequirement } from "../types";

const store = useProjectStore();
const { pendingDependencies: plan } = storeToRefs(store);

function describe(u: UnmetRequirement): string {
  const want = u.range ? `${u.target} ${u.range}` : u.target;
  return u.status === "missing"
    ? `${u.consumer} requires ${want}, which isn't in your Shipments`
    : `${u.consumer} requires ${want}, and the installed version is outside that range`;
}

function confirm() {
  if (plan.value?.install.length) void store.confirmShipmentDependencies();
  else store.dismissShipmentDependencies();
}
</script>

<template>
  <ConfirmDialog
    :open="!!plan"
    title="Shipment requirements aren't met"
    :confirm-label="plan?.install.length ? `Add ${plan.install.length} from mercs.ink` : 'OK'"
    :cancel-label="plan?.install.length ? 'Not now' : 'Close'"
    @confirm="confirm"
    @cancel="store.dismissShipmentDependencies()"
  >
    <template v-if="plan">
      <ul class="list-inside list-disc text-amber-200">
        <li v-for="u in plan.unmet" :key="`${u.consumer}->${u.target}`">{{ describe(u) }}</li>
      </ul>

      <div
        v-if="plan.install.length"
        class="mt-3 rounded-lg border border-zinc-700 bg-zinc-800/60 px-3 py-2 text-zinc-300"
      >
        <p class="font-medium">Add to your Shipments from mercs.ink:</p>
        <ul class="mt-1 list-inside list-disc">
          <li v-for="d in plan.install" :key="d.name">
            <span class="text-zinc-100">{{ d.name }} {{ d.release_version }}</span>
            <span v-if="d.updated_from" class="text-zinc-400"> (update from {{ d.updated_from }})</span>
            <span v-if="d.required_by.length" class="text-zinc-500">
              — required by {{ d.required_by.join(", ") }}</span
            >
          </li>
        </ul>
      </div>

      <p
        v-if="plan.unresolved"
        class="mt-3 rounded-lg border border-red-600/30 bg-red-500/10 px-3 py-2 text-red-200"
      >
        mercs.ink can't meet these: {{ plan.unresolved }}
      </p>
    </template>
  </ConfirmDialog>
</template>
