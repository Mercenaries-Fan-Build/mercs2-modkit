<script setup lang="ts">
/**
 * The texture itself: zoomable, pannable, and — the useful part — with the model's UV layout
 * drawn on top.
 *
 * # Why the UV overlay matters
 *
 * A texture like `vz_angel_falls_tiny_tinygeometry_*` is an **atlas**: dozens of unrelated
 * little images packed into one 1024×1024 sheet, shared by 34 models. Looking at the sheet
 * tells you nothing about which square belongs to what. The UV coordinates are the answer —
 * they are literally the map from a model's triangles onto this image. Drawing the triangles
 * of the parts that use the texture shows you exactly which region of the atlas a given model
 * occupies, which is the thing you need to know before painting over any of it.
 *
 * The wireframe comes from geometry we already fetch for the 3D view (`uvs` + `indices` +
 * per-group `uses_texture`), so it costs nothing extra.
 */
import { ref, computed, watch, onMounted, onBeforeUnmount } from "vue";
import type { ModelGeometry } from "../types";

const props = defineProps<{
  /** Decoded texture image. */
  src: string;
  /** Texture's real dimensions (the image may be a smaller resident mip). */
  width: number;
  height: number;
  /** Optional geometry whose UVs are drawn over the image. */
  geometry?: ModelGeometry | null;
  /** Draw only this part's UVs (a draw group id). Null = every part that uses the texture. */
  selected?: number | null;
}>();

const canvas = ref<HTMLCanvasElement | null>(null);
const wrap = ref<HTMLDivElement | null>(null);

const zoom = ref(1);
const panX = ref(0);
const panY = ref(0);
const showUvs = ref(true);
const dragging = ref(false);
let last = { x: 0, y: 0 };

const img = new Image();
const loaded = ref(false);
img.onload = () => {
  loaded.value = true;
  draw();
};
watch(() => props.src, (s) => {
  loaded.value = false;
  img.src = s;
}, { immediate: true });

/**
 * Which parts' UVs to draw.
 *
 * Only ever parts that actually use THIS texture. A part bound to a different material maps
 * into *that* texture's UV space, so drawing its islands over this image would be nonsense —
 * the same coordinates mean something else there.
 */
const uvGroups = computed(() => {
  const all = props.geometry?.groups.filter((g) => g.uses_texture) ?? [];
  if (props.selected == null) return all;
  return all.filter((g) => g.id === props.selected);
});

/** The user selected a part that doesn't use this texture — say so instead of drawing nothing. */
const selectedUsesOther = computed(
  () =>
    props.selected != null &&
    props.geometry?.groups.some((g) => g.id === props.selected && !g.uses_texture) === true,
);

function draw() {
  const c = canvas.value;
  if (!c || !loaded.value) return;
  const ctx = c.getContext("2d");
  if (!ctx) return;

  const w = c.width;
  const h = c.height;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, w, h);

  // Fit the (square-ish) texture into the canvas, then apply the user's zoom/pan.
  const base = Math.min(w, h);
  ctx.translate(panX.value, panY.value);
  ctx.scale(zoom.value, zoom.value);

  // A checker under the image, so transparent regions of an atlas read as transparent
  // instead of as black.
  const sq = 8;
  for (let y = 0; y < base; y += sq) {
    for (let x = 0; x < base; x += sq) {
      ctx.fillStyle = ((x / sq + y / sq) & 1) === 0 ? "#1a1a1d" : "#232327";
      ctx.fillRect(x, y, sq, sq);
    }
  }

  ctx.imageSmoothingEnabled = zoom.value < 4; // go crisp/nearest when zoomed right in
  ctx.drawImage(img, 0, 0, base, base);

  if (!showUvs.value || !props.geometry) return;

  const { uvs, indices } = props.geometry;
  ctx.lineWidth = Math.max(0.4, 0.8 / zoom.value);
  ctx.strokeStyle = "rgba(16, 185, 129, 0.85)"; // emerald, matching the 3D highlight
  ctx.beginPath();
  for (const g of uvGroups.value) {
    const end = g.index_start + g.index_count;
    for (let i = g.index_start; i + 2 < end; i += 3) {
      const a = indices[i] * 2;
      const b = indices[i + 1] * 2;
      const cIdx = indices[i + 2] * 2;
      // UV origin is top-left, same as the image — no V flip.
      const ax = uvs[a] * base;
      const ay = uvs[a + 1] * base;
      const bx = uvs[b] * base;
      const by = uvs[b + 1] * base;
      const cx = uvs[cIdx] * base;
      const cy = uvs[cIdx + 1] * base;
      ctx.moveTo(ax, ay);
      ctx.lineTo(bx, by);
      ctx.lineTo(cx, cy);
      ctx.closePath();
    }
  }
  ctx.stroke();
}

function resize() {
  const c = canvas.value;
  const el = wrap.value;
  if (!c || !el) return;
  const size = Math.min(el.clientWidth, 520);
  c.width = size;
  c.height = size;
  draw();
}

function onWheel(e: WheelEvent) {
  e.preventDefault();
  const c = canvas.value;
  if (!c) return;
  const r = c.getBoundingClientRect();
  const mx = e.clientX - r.left;
  const my = e.clientY - r.top;

  const next = Math.min(24, Math.max(1, zoom.value * (e.deltaY < 0 ? 1.15 : 1 / 1.15)));
  // Zoom about the cursor, not the origin — otherwise zooming in on a corner of an atlas
  // walks the region you're aiming at straight off the canvas.
  panX.value = mx - ((mx - panX.value) / zoom.value) * next;
  panY.value = my - ((my - panY.value) / zoom.value) * next;
  zoom.value = next;
  clampPan();
  draw();
}

function clampPan() {
  const c = canvas.value;
  if (!c) return;
  const span = Math.min(c.width, c.height) * zoom.value;
  panX.value = Math.min(0, Math.max(c.width - span, panX.value));
  panY.value = Math.min(0, Math.max(c.height - span, panY.value));
}

function onDown(e: PointerEvent) {
  dragging.value = true;
  last = { x: e.clientX, y: e.clientY };
  (e.target as HTMLElement).setPointerCapture(e.pointerId);
}
function onMove(e: PointerEvent) {
  if (!dragging.value) return;
  panX.value += e.clientX - last.x;
  panY.value += e.clientY - last.y;
  last = { x: e.clientX, y: e.clientY };
  clampPan();
  draw();
}
function onUp() {
  dragging.value = false;
}

function reset() {
  zoom.value = 1;
  panX.value = 0;
  panY.value = 0;
  draw();
}

watch([() => props.geometry, () => props.selected, showUvs], draw);

let ro: ResizeObserver | null = null;
onMounted(() => {
  resize();
  ro = new ResizeObserver(resize);
  if (wrap.value) ro.observe(wrap.value);
});
onBeforeUnmount(() => ro?.disconnect());
</script>

<template>
  <div ref="wrap" class="w-full">
    <canvas
      ref="canvas"
      class="rounded-lg border border-zinc-800 bg-black/40"
      :class="dragging ? 'cursor-grabbing' : 'cursor-grab'"
      @wheel="onWheel"
      @pointerdown="onDown"
      @pointermove="onMove"
      @pointerup="onUp"
      @pointercancel="onUp"
    />

    <div class="mt-2 flex flex-wrap items-center gap-2 text-xs">
      <span class="text-zinc-500">{{ Math.round(zoom * 100) }}%</span>
      <button
        class="rounded border border-zinc-700 px-2 py-0.5 text-zinc-400 hover:bg-zinc-800"
        @click="reset"
      >
        Reset
      </button>

      <label
        v-if="geometry"
        class="ml-1 flex items-center gap-1.5 text-zinc-400"
        :title="`${uvGroups.length} part(s) of this model are mapped onto this texture`"
      >
        <input v-model="showUvs" type="checkbox" class="accent-emerald-500" />
        Show where the model sits on it
      </label>

      <span class="ml-auto text-zinc-600">scroll to zoom · drag to pan</span>
    </div>

    <p v-if="selectedUsesOther" class="mt-1 text-[11px] text-amber-300/80">
      That part uses a different texture, so it isn't mapped onto this one.
    </p>
    <p
      v-else-if="geometry && uvGroups.length"
      class="mt-1 text-[11px] text-zinc-600"
    >
      The green outline is the UV layout of
      {{ selected == null ? "the parts that use this texture" : "that part" }} — the exact
      patch of this image its triangles are cut from.
    </p>
  </div>
</template>
