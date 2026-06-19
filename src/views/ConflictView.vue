<script setup lang="ts">
import { storeToRefs } from "pinia";
import {
  RadioGroup,
  RadioGroupLabel,
  RadioGroupOption,
} from "@headlessui/vue";
import { useProjectStore } from "../stores/project";
import type { AssetConflict, Resolution } from "../types";

const store = useProjectStore();
const { conflictGraph } = storeToRefs(store);

function hex(n: number): string {
  return "0x" + (n >>> 0).toString(16).toUpperCase().padStart(8, "0");
}

/** Build the selectable options for a conflict: one per mod, plus "exclude". */
function optionsFor(c: AssetConflict): { key: string; label: string; res: Resolution }[] {
  const opts = c.mods.map((modId) => ({
    key: `priority:${modId}`,
    label: `Use ${modId}`,
    res: { kind: "priority", modId } as Resolution,
  }));
  opts.push({
    key: "exclude",
    label: "Exclude from build",
    res: { kind: "exclude", modId: c.mods[0] } as Resolution,
  });
  return opts;
}

function currentKey(assetHash: number): string {
  const res = store.resolutions[String(assetHash)];
  if (!res) return "";
  return res.kind === "priority" ? `priority:${res.modId}` : "exclude";
}

function choose(c: AssetConflict, key: string) {
  const opt = optionsFor(c).find((o) => o.key === key);
  if (opt) store.setResolution(c.asset_hash, opt.res);
}
</script>

<template>
  <div class="mx-auto max-w-3xl px-8 py-6">
    <header>
      <h2 class="text-xl font-semibold">Conflicts</h2>
      <p class="text-sm text-zinc-500">
        Assets claimed by more than one mod. The engine applies last-write-wins;
        choose which mod owns each asset, or exclude it.
      </p>
    </header>

    <div
      v-if="!conflictGraph || conflictGraph.conflicts.length === 0"
      class="mt-10 rounded-xl border border-dashed border-zinc-800 px-8 py-16 text-center"
    >
      <p class="text-emerald-400">No conflicts 🎉</p>
      <p class="mt-1 text-sm text-zinc-600">
        Every asset across your loaded mods is unique.
      </p>
    </div>

    <div v-else class="mt-6 space-y-4">
      <div
        v-for="c in conflictGraph.conflicts"
        :key="c.asset_hash"
        class="rounded-xl border border-zinc-800 bg-zinc-900/50 p-4"
      >
        <div class="mb-3 flex items-center justify-between">
          <div>
            <p class="font-medium text-zinc-100">
              {{ c.asset_name ?? "Unknown asset" }}
            </p>
            <p class="font-mono text-xs text-zinc-500">{{ hex(c.asset_hash) }}</p>
          </div>
          <span
            class="rounded-full bg-amber-500/15 px-2 py-0.5 text-xs text-amber-300"
          >
            {{ c.mods.length }} mods
          </span>
        </div>

        <RadioGroup
          :model-value="currentKey(c.asset_hash)"
          @update:model-value="(k: string) => choose(c, k)"
        >
          <RadioGroupLabel class="sr-only">
            Resolution for {{ c.asset_name ?? hex(c.asset_hash) }}
          </RadioGroupLabel>
          <div class="flex flex-wrap gap-2">
            <RadioGroupOption
              v-for="opt in optionsFor(c)"
              :key="opt.key"
              :value="opt.key"
              v-slot="{ checked }"
            >
              <span
                class="cursor-pointer rounded-lg border px-3 py-1.5 text-sm transition"
                :class="
                  checked
                    ? 'border-emerald-500 bg-emerald-500/15 text-emerald-200'
                    : 'border-zinc-700 text-zinc-400 hover:border-zinc-600'
                "
              >
                {{ opt.label }}
              </span>
            </RadioGroupOption>
          </div>
        </RadioGroup>
      </div>
    </div>
  </div>
</template>
