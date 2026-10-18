<script setup lang="ts">
/**
 * Placements whose displaced file's `.bak` is gone. Every install and "Remove the patch" undoes the
 * previous placements first and cannot move these back, so both refuse until the player forgets
 * them here: per Shipment, or all at once when more than one Shipment has some.
 */
import { computed, ref } from "vue";
import { storeToRefs } from "pinia";
import { useProjectStore } from "../stores/project";
import ConfirmDialog from "./ConfirmDialog.vue";
import type { LostBackup } from "../types";

const store = useProjectStore();
const { lostBackups, busy } = storeToRefs(store);

const byShipment = computed(() => {
  const groups = new Map<string, LostBackup[]>();
  for (const l of lostBackups.value) groups.set(l.shipment, [...(groups.get(l.shipment) ?? []), l]);
  return [...groups].map(([shipment, files]) => ({ shipment, files }));
});

/** The Shipment being confirmed, `null` for all, `undefined` when no dialog is open. */
const confirming = ref<string | null | undefined>(undefined);
const confirmFiles = computed(() =>
  confirming.value === undefined
    ? []
    : lostBackups.value.filter((l) => confirming.value === null || l.shipment === confirming.value),
);

function forget() {
  const shipment = confirming.value;
  confirming.value = undefined;
  if (shipment !== undefined) void store.forgetLostBackups(shipment);
}
</script>

<template>
  <section
    v-if="lostBackups.length"
    class="mt-4 rounded-lg border border-red-600/30 bg-red-500/10 px-4 py-3 text-sm text-red-200"
  >
    <p>
      An earlier install renamed a file that was already in your game folder to <code>.bak</code>,
      and that <code>.bak</code> is gone, so it can't be put back. Installing and removing the patch
      are stopped until you forget it. Modkit's own copy still comes out on the next install.
    </p>
    <ul class="mt-3 space-y-2">
      <li
        v-for="g in byShipment"
        :key="g.shipment"
        class="flex items-start gap-3 rounded-lg border border-red-600/20 bg-black/20 px-3 py-2"
      >
        <div class="min-w-0 flex-1">
          <p class="text-red-100">{{ g.shipment }}</p>
          <p v-for="f in g.files" :key="f.relative" class="truncate font-mono text-xs text-red-300/80">
            {{ f.relative }} — missing {{ f.backup }}
          </p>
        </div>
        <button class="btn-outline shrink-0" :disabled="busy" @click="confirming = g.shipment">
          Forget
        </button>
      </li>
    </ul>
    <button
      v-if="byShipment.length > 1"
      class="mt-3 rounded-lg bg-red-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-red-500"
      :disabled="busy"
      @click="confirming = null"
    >
      Forget all
    </button>
    <ConfirmDialog
      :open="confirming !== undefined"
      title="Forget the missing backups?"
      confirm-label="Forget"
      danger
      @confirm="forget"
      @cancel="confirming = undefined"
    >
      <p>Modkit stops trying to put back the file{{ confirmFiles.length === 1 ? "" : "s" }} it renamed for:</p>
      <ul class="mt-2 space-y-1 font-mono text-xs">
        <li v-for="f in confirmFiles" :key="f.relative">{{ f.relative }}</li>
      </ul>
      <p class="mt-2">
        The renamed file{{ confirmFiles.length === 1 ? " is" : "s are" }} already gone, so nothing in
        your game folder changes now.
      </p>
    </ConfirmDialog>
  </section>
</template>
