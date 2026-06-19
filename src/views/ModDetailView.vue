<script setup lang="ts">
import { computed } from "vue";
import { useRouter } from "vue-router";
import { useProjectStore } from "../stores/project";

const props = defineProps<{ id: string }>();
const store = useProjectStore();
const router = useRouter();

const mod = computed(() => store.modById(props.id));

function hex(n: number): string {
  return "0x" + (n >>> 0).toString(16).toUpperCase().padStart(8, "0");
}
</script>

<template>
  <div v-if="mod" class="mx-auto max-w-4xl px-8 py-6">
    <button
      class="text-sm text-zinc-500 hover:text-zinc-300"
      @click="router.back()"
    >
      ← Back
    </button>

    <header class="mt-2">
      <h2 class="text-xl font-semibold">{{ mod.manifest.name }}</h2>
      <p class="text-sm text-zinc-500">
        v{{ mod.manifest.version }}
        <span v-if="mod.manifest.author"> · {{ mod.manifest.author }}</span>
      </p>
      <p v-if="mod.manifest.description" class="mt-2 text-sm text-zinc-400">
        {{ mod.manifest.description }}
      </p>
    </header>

    <section class="mt-6">
      <h3 class="mb-2 text-sm font-medium text-zinc-300">
        Assets ({{ mod.assets.length }})
      </h3>
      <div class="overflow-hidden rounded-xl border border-zinc-800">
        <table class="w-full text-left text-sm">
          <thead class="bg-zinc-900/60 text-xs uppercase text-zinc-500">
            <tr>
              <th class="px-4 py-2 font-medium">Name</th>
              <th class="px-4 py-2 font-medium">Type</th>
              <th class="px-4 py-2 font-medium">Patch</th>
              <th class="px-4 py-2 font-medium">Hash</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-zinc-800">
            <tr v-for="a in mod.assets" :key="a.asset_hash">
              <td class="px-4 py-2 text-zinc-200">{{ a.name }}</td>
              <td class="px-4 py-2">
                <span class="rounded bg-zinc-800 px-1.5 py-0.5 text-xs">
                  {{ a.detected_type }}
                </span>
              </td>
              <td class="px-4 py-2 text-zinc-400">{{ a.target_patch }}</td>
              <td class="px-4 py-2 font-mono text-xs text-zinc-500">
                {{ hex(a.asset_hash) }}
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
  </div>

  <div v-else class="px-8 py-16 text-center text-zinc-500">
    Mod not found.
  </div>
</template>
