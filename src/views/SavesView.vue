<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { storeToRefs } from "pinia";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { useProjectStore } from "../stores/project";
import type { SaveBackupInfo } from "../types";
import ConfirmDialog from "../components/ConfirmDialog.vue";
import Spinner from "../components/Spinner.vue";

const store = useProjectStore();
const { savesInfo, saveBackups, savesBusy, error, gameRunning } =
  storeToRefs(store);

// One-line outcome of the last manual backup ("Backed up 3 saves", "Skipped —
// identical to the latest backup"). Cleared on the next action.
const lastAction = ref<string | null>(null);

const pendingRestore = ref<SaveBackupInfo | null>(null);
const pendingDelete = ref<SaveBackupInfo | null>(null);

onMounted(() => {
  store.refreshSaves().catch(() => {});
});

/** Pick a different SaveGames folder (persisted; also used by auto-backup). */
async function changeSavesDir() {
  const dir = await open({
    directory: true,
    title: "Choose the SaveGames folder",
    defaultPath: savesInfo.value?.dir ?? undefined,
  });
  if (typeof dir === "string" && dir) {
    lastAction.value = null;
    await store.setSavesDir(dir).catch(() => {});
  }
}

async function useDefaultSavesDir() {
  lastAction.value = null;
  await store.setSavesDir(null).catch(() => {});
}

async function backupNow() {
  lastAction.value = null;
  const res = await store.backupSavesNow().catch(() => null);
  if (!res) return;
  lastAction.value = res.id
    ? `Backed up ${res.file_count} save${res.file_count === 1 ? "" : "s"}.`
    : `Skipped — ${res.skipped}.`;
}

async function confirmRestore() {
  const backup = pendingRestore.value;
  pendingRestore.value = null;
  if (!backup) return;
  lastAction.value = null;
  const res = await store.restoreSaveBackup(backup.id).catch(() => null);
  if (!res) return;
  lastAction.value = `Restored ${res.restored.length} save${
    res.restored.length === 1 ? "" : "s"
  } from ${formatDate(backup.created_unix)}.`;
}

async function confirmDelete() {
  const backup = pendingDelete.value;
  pendingDelete.value = null;
  if (!backup) return;
  lastAction.value = null;
  await store.deleteSaveBackup(backup.id).catch(() => {});
}

const hasSaves = computed(() => (savesInfo.value?.saves.length ?? 0) > 0);

function formatDate(unix: number | null): string {
  if (!unix) return "—";
  return new Date(unix * 1000).toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
}

function formatPlaytime(seconds: number | null): string {
  if (seconds == null) return "—";
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return `${h}h ${String(m).padStart(2, "0")}m`;
}

function formatCash(cash: number | null): string {
  return cash == null ? "—" : `$${cash.toLocaleString()}`;
}

function formatBytes(bytes: number): string {
  return bytes >= 1024 * 1024
    ? `${(bytes / (1024 * 1024)).toFixed(1)} MB`
    : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/** "pre-launch" → "before launch", etc., for the backups table. */
function reasonLabel(reason: string): string {
  return (
    { "pre-launch": "before launch", "pre-restore": "before restore", manual: "manual" }[
      reason
    ] ?? reason
  );
}
</script>

<template>
  <div class="mx-auto max-w-3xl px-8 py-6">
    <header>
      <h2 class="plate-title text-xl">Save backups</h2>
      <p class="text-sm text-zinc-500">
        Mercenaries 2 constantly overwrites its autosave slot, so a crash or a
        misbehaving mod can wipe out a playthrough. Modkit snapshots your saves
        automatically every time it launches the game; you can also back up and
        restore manually here.
      </p>
    </header>

    <p
      v-if="error"
      class="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300"
    >
      {{ error }}
    </p>

    <!-- Current saves -->
    <section class="mt-5 rounded-xl border border-zinc-800 p-5">
      <div class="flex items-center justify-between gap-3">
        <h3 class="plate-title text-sm">Current saves</h3>
        <div class="flex items-center gap-2">
          <button
            v-if="savesInfo?.exists"
            class="btn-outline"
            @click="savesInfo?.dir && openPath(savesInfo.dir)"
          >
            Open folder
          </button>
          <button
            class="rounded-lg px-3 py-2 text-sm text-zinc-300 hover:bg-zinc-800"
            title="Point modkit at a different SaveGames folder"
            @click="changeSavesDir"
          >
            Change folder…
          </button>
          <button
            class="btn-plate"
            :disabled="savesBusy || !hasSaves"
            @click="backupNow"
          >
            <Spinner v-if="savesBusy" />
            Back up now
          </button>
        </div>
      </div>

      <!-- Where modkit looks for saves: autodetected by default, changeable. -->
      <p
        class="mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-zinc-500"
      >
        <span class="shrink-0">Saves folder:</span>
        <code
          class="max-w-full truncate rounded bg-zinc-900 px-1.5 py-0.5 text-zinc-300"
          :title="savesInfo?.dir ?? ''"
          >{{ savesInfo?.dir ?? "detecting…" }}</code
        >
        <span
          v-if="savesInfo?.overridden"
          class="rounded-full border border-sky-600/30 bg-sky-500/10 px-2 py-0.5 text-sky-300"
          title="You picked this folder; modkit is not autodetecting it"
        >
          custom
        </span>
        <button
          v-if="savesInfo?.overridden"
          class="text-zinc-300 underline hover:text-white"
          @click="useDefaultSavesDir"
        >
          Use default
        </button>
      </p>

      <p v-if="lastAction" class="mt-3 text-sm text-emerald-300">
        {{ lastAction }}
      </p>

      <p v-if="!savesInfo?.exists" class="mt-3 text-sm text-zinc-500">
        This folder doesn't exist yet — it appears after the game saves for the
        first time. If your saves live somewhere else, use “Change…” above.
      </p>
      <p v-else-if="!hasSaves" class="mt-3 text-sm text-zinc-500">
        The SaveGames folder is empty.
      </p>

      <table v-if="hasSaves" class="mt-4 w-full text-left text-sm">
        <thead class="text-xs uppercase tracking-wide text-zinc-500">
          <tr>
            <th class="pb-2 pr-3 font-medium">Character</th>
            <th class="pb-2 pr-3 font-medium">Cash</th>
            <th class="pb-2 pr-3 font-medium">Playtime</th>
            <th class="pb-2 pr-3 font-medium">Saved</th>
            <th class="pb-2 font-medium">File</th>
          </tr>
        </thead>
        <tbody class="divide-y divide-zinc-800/60 text-zinc-300">
          <tr v-for="save in savesInfo!.saves" :key="save.file_name">
            <td class="py-2 pr-3">
              {{ save.character || "—" }}
              <span
                v-if="save.autosave"
                class="stamp ml-1.5 text-amber-300"
                title="The game's rolling autosave slot — overwritten constantly"
              >
                autosave
              </span>
            </td>
            <td class="py-2 pr-3">{{ formatCash(save.cash) }}</td>
            <td class="py-2 pr-3">{{ formatPlaytime(save.playtime_seconds) }}</td>
            <td class="py-2 pr-3">{{ formatDate(save.saved_at_unix) }}</td>
            <td class="py-2 font-mono text-xs text-zinc-500">
              {{ save.file_name }}
            </td>
          </tr>
        </tbody>
      </table>
    </section>

    <!-- Stored snapshots -->
    <section class="mt-5 rounded-xl border border-zinc-800 p-5">
      <h3 class="plate-title text-sm">Backups</h3>
      <p class="mt-1 text-sm text-zinc-400">
        Newest first. Restoring copies a snapshot back over the SaveGames
        folder — the current saves are snapshotted first, so a restore is
        always undoable. Identical snapshots are skipped, and only the most
        recent 30 are kept.
      </p>

      <p v-if="saveBackups.length === 0" class="mt-3 text-sm text-zinc-500">
        No backups yet. One is taken automatically each time you launch the
        game from modkit.
      </p>

      <table v-else class="mt-4 w-full text-left text-sm">
        <thead class="text-xs uppercase tracking-wide text-zinc-500">
          <tr>
            <th class="pb-2 pr-3 font-medium">Taken</th>
            <th class="pb-2 pr-3 font-medium">Trigger</th>
            <th class="pb-2 pr-3 font-medium">Characters</th>
            <th class="pb-2 pr-3 font-medium">Size</th>
            <th class="pb-2 font-medium"></th>
          </tr>
        </thead>
        <tbody class="divide-y divide-zinc-800/60 text-zinc-300">
          <tr v-for="backup in saveBackups" :key="backup.id">
            <td class="py-2 pr-3">{{ formatDate(backup.created_unix) }}</td>
            <td class="py-2 pr-3 text-zinc-400">
              {{ reasonLabel(backup.reason) }}
            </td>
            <td class="py-2 pr-3">
              {{ backup.characters.join(", ") || "?" }}
              <span class="text-xs text-zinc-500">
                ({{ backup.file_count }} file{{
                  backup.file_count === 1 ? "" : "s"
                }})
              </span>
            </td>
            <td class="py-2 pr-3 text-zinc-400">
              {{ formatBytes(backup.total_bytes) }}
            </td>
            <td class="py-2 text-right whitespace-nowrap">
              <button
                class="btn-outline"
                :disabled="savesBusy || gameRunning"
                :title="
                  gameRunning
                    ? 'Stop the game before restoring saves'
                    : 'Copy this snapshot back over the SaveGames folder'
                "
                @click="pendingRestore = backup"
              >
                Restore
              </button>
              <button
                class="ml-2 rounded-lg border border-red-900/50 px-3 py-1.5 text-xs text-red-400 hover:bg-red-500/10 disabled:opacity-50"
                :disabled="savesBusy"
                @click="pendingDelete = backup"
              >
                Delete
              </button>
            </td>
          </tr>
        </tbody>
      </table>
    </section>

    <!-- Restore confirmation -->
    <ConfirmDialog
      :open="!!pendingRestore"
      :title="`Restore saves from ${formatDate(pendingRestore?.created_unix ?? null)}?`"
      confirm-label="Back up current & restore"
      @confirm="confirmRestore"
      @cancel="pendingRestore = null"
    >
      <p>
        This copies
        <strong class="text-zinc-200">
          {{ pendingRestore?.file_count }} save file{{
            pendingRestore?.file_count === 1 ? "" : "s"
          }}
        </strong>
        back into the SaveGames folder, overwriting the current saves. The
        current saves are backed up first, so you can undo this.
      </p>
    </ConfirmDialog>

    <!-- Delete confirmation -->
    <ConfirmDialog
      :open="!!pendingDelete"
      :title="`Delete the ${formatDate(pendingDelete?.created_unix ?? null)} backup?`"
      confirm-label="Delete backup"
      danger
      @confirm="confirmDelete"
      @cancel="pendingDelete = null"
    >
      <p>
        This permanently deletes the snapshot
        <template v-if="pendingDelete?.characters.length">
          for
          <strong class="text-zinc-200">{{
            pendingDelete.characters.join(", ")
          }}</strong>
        </template>
        — it is not moved to the trash.
      </p>
    </ConfirmDialog>
  </div>
</template>
