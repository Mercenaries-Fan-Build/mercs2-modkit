<script setup lang="ts">
import { computed, onMounted } from "vue";
import { RouterLink, RouterView } from "vue-router";
import { storeToRefs } from "pinia";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useProjectStore } from "./stores/project";
import { useGamepadNavigation } from "./composables/useGamepadNavigation";
import GameBar from "./components/GameBar.vue";

const store = useProjectStore();
const { mods, asiMods, conflictCount, modkitUpdate } = storeToRefs(store);

// Drive the whole UI from a controller when no keyboard is handy.
const { connected: padConnected, controllerId } = useGamepadNavigation();

// Controller ids look like "Xbox 360 Controller (STANDARD GAMEPAD ...)" —
// keep just the human-readable name before the parenthetical.
const padName = computed(
  () => controllerId.value?.split(" (")[0]?.trim() || "Controller"
);

onMounted(() => {
  store.init();
  store.checkModkitUpdate();
});
</script>

<template>
  <div class="flex h-full bg-zinc-950 text-zinc-100">
    <!-- Sidebar -->
    <aside
      class="flex w-60 shrink-0 flex-col border-r border-zinc-800 bg-zinc-900/60"
    >
      <div class="px-5 py-4 border-b border-zinc-800">
        <h1 class="text-lg font-semibold tracking-tight">mercs2 modkit</h1>
        <p class="text-xs text-zinc-500">Mercenaries 2 mod manager</p>
      </div>

      <nav class="flex-1 space-y-1 px-3 py-4 text-sm">
        <RouterLink to="/" class="nav-link" active-class="nav-link-active">
          Library
          <span class="ml-auto rounded-full bg-zinc-800 px-2 text-xs">
            {{ mods.length + asiMods.length }}
          </span>
        </RouterLink>
        <RouterLink
          to="/catalog"
          class="nav-link"
          active-class="nav-link-active"
        >
          Browse
        </RouterLink>
        <RouterLink
          v-if="!store.gameFullySetUp"
          to="/setup"
          class="nav-link"
          active-class="nav-link-active"
        >
          Setup
        </RouterLink>
        <RouterLink
          to="/conflicts"
          class="nav-link"
          active-class="nav-link-active"
        >
          Conflicts
          <span
            v-if="conflictCount"
            class="ml-auto rounded-full bg-amber-500/20 px-2 text-xs text-amber-300"
          >
            {{ conflictCount }}
          </span>
        </RouterLink>
        <RouterLink
          to="/export"
          class="nav-link"
          active-class="nav-link-active"
        >
          Build &amp; Deploy
        </RouterLink>
        <RouterLink
          to="/diagnostics"
          class="nav-link"
          active-class="nav-link-active"
        >
          Diagnostics
        </RouterLink>
      </nav>

      <div
        v-if="padConnected"
        class="flex items-center gap-2 px-5 py-2 text-xs border-t border-zinc-800 text-emerald-400"
        :title="`${padName} connected — navigate with the D-pad/stick, A to select, B to go back`"
      >
        <!-- gamepad glyph -->
        <svg
          class="h-4 w-4 shrink-0"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <line x1="6" y1="11" x2="10" y2="11" />
          <line x1="8" y1="9" x2="8" y2="13" />
          <line x1="15" y1="11" x2="15.01" y2="11" />
          <line x1="18" y1="9" x2="18.01" y2="9" />
          <path
            d="M17.32 5H6.68a4 4 0 0 0-3.978 3.59c-.006.052-.01.101-.017.152C2.604 9.416 2 14.456 2 16a3 3 0 0 0 3 3c1 0 1.5-.5 2-1l1.414-1.414A2 2 0 0 1 9.828 16h4.344a2 2 0 0 1 1.414.586L17 18c.5.5 1 1 2 1a3 3 0 0 0 3-3c0-1.544-.604-6.584-.685-7.258-.007-.05-.011-.1-.017-.151A4 4 0 0 0 17.32 5z"
          />
        </svg>
        <span class="truncate">{{ padName }} connected</span>
      </div>
      <div v-else>No controller connected</div>

      <div class="px-5 py-3 text-xs border-t border-zinc-800">
        <button
          v-if="modkitUpdate?.available"
          class="flex items-center gap-1.5 font-medium text-emerald-400 hover:underline"
          :title="`You have v${modkitUpdate.current}. Open the ${modkitUpdate.latest} release page.`"
          @click="openUrl(modkitUpdate.url)"
        >
          <span class="h-1.5 w-1.5 rounded-full bg-emerald-400" />
          Update available → {{ modkitUpdate.latest }}
        </button>
        <span v-else class="text-zinc-600">
          v{{ modkitUpdate?.current ?? "0.1.0" }}
        </span>
      </div>
    </aside>

    <!-- Main content -->
    <div class="flex flex-1 flex-col overflow-hidden">
      <GameBar />
      <main class="flex-1 overflow-y-auto">
        <RouterView />
      </main>
    </div>
  </div>
</template>

<style>
.nav-link {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  border-radius: 0.5rem;
  padding: 0.5rem 0.75rem;
  color: rgb(161 161 170);
  transition: background-color 0.15s, color 0.15s;
}
.nav-link:hover {
  background-color: rgb(39 39 42 / 0.6);
  color: rgb(244 244 245);
}
.nav-link-active {
  background-color: rgb(39 39 42);
  color: rgb(255 255 255);
}
</style>
