<script setup lang="ts">
import { ref, shallowRef, onMounted, onBeforeUnmount, watch } from "vue";
import * as THREE from "three";
import { useProjectStore } from "../stores/project";
import type { ModelGeometry } from "../types";

/**
 * A hero rendered as an engraved bust inside the oval cartouche.
 *
 * The game's own shell does exactly this — `data/Movies/shell_mattias.bik` and
 * its Jennifer/Chris siblings frame each playable character as a portrait in an
 * oval, over guilloche. Those are Bink videos, and the game files are
 * copyrighted (`/storage/` is gitignored), so nothing is shipped with modkit.
 * Instead we read the character's model out of the player's own `vz.wad`,
 * decode its skins, and render it here — same portrait, their copy.
 *
 * Static, not interactive: this renders one frame and stops. Three of these sit
 * on the picker at once and none of them needs a render loop. The single render
 * is deliberately deferred until every skin has decoded, since there's no
 * second frame to correct a half-textured first one.
 */
const props = defineProps<{
  /** Model name in the WAD, e.g. `pmc_hum_mattias`. */
  model: string;
  /** Shown while loading, and if the model can't be read. */
  fallback: string;
  active: boolean;
}>();

/**
 * Where the bust sits, as fractions of the model's total height. A standing
 * humanoid carries its head in roughly the top eighth, so we aim high and frame
 * tight. These two are the knobs to turn if a portrait comes out showing chest
 * instead of face: raise EYE_LINE to climb, lower FRAME to zoom in.
 */
const EYE_LINE = 0.86;
const FRAME = 0.19;

/**
 * Reading a portrait means indexing vz.wad (2.5 GB) and decoding skins. The
 * backend commands are `#[tauri::command(async)]` so that work is off the UI
 * thread, but three heroes mounting at once would still have three of those
 * indexing passes running concurrently. Serialise them through one chain: the
 * portraits then fill in one after another instead of contending.
 */
let chain: Promise<unknown> = Promise.resolve();
function enqueue<T>(job: () => Promise<T>): Promise<T> {
  const next = chain.then(job, job);
  // Keep the chain alive even when a job rejects, and don't leave an unhandled
  // rejection behind on the copy we store.
  chain = next.catch(() => undefined);
  return next;
}

/** Decoded portrait payload, kept so revisiting the Wardrobe is instant. */
type Portrait = { geo: ModelGeometry; urls: Map<string, string> };
const cache = new Map<string, Portrait>();

async function fetchPortrait(store: ReturnType<typeof useProjectStore>, model: string) {
  const hit = cache.get(model);
  if (hit) return hit;

  const geo = await store.modelGeometry(model, "");

  const wanted = new Set<string>();
  for (const grp of geo.groups) {
    const d = grp.textures.find((t) => t.slot === "diffuse" && t.name);
    if (d?.name) wanted.add(d.name);
  }

  const urls = new Map<string, string>();
  for (const name of wanted) {
    try {
      const url = (await store.textureDetails(name)).preview?.data_url;
      if (url) urls.set(name, url);
    } catch {
      // Streamed-out or unreadable; that part falls back to the paper tone.
    }
  }

  const payload = { geo, urls };
  cache.set(model, payload);
  return payload;
}

const store = useProjectStore();
const host = ref<HTMLDivElement | null>(null);
const ready = ref(false);
const renderer = shallowRef<THREE.WebGLRenderer | null>(null);
const textures = shallowRef<THREE.Texture[]>([]);

function teardown() {
  // Decoded skins hold GPU memory, and three of these live on the picker at
  // once — drop them explicitly rather than waiting on GC.
  textures.value.forEach((t) => t.dispose());
  textures.value = [];
  const r = renderer.value;
  if (r) {
    r.dispose();
    r.domElement.remove();
    renderer.value = null;
  }
  ready.value = false;
}

async function draw() {
  const el = host.value;
  if (!el || !store.gamePath) return;

  let portrait: Portrait;
  try {
    // The texture argument is the one to *highlight*, which a portrait has no
    // use for — pass an empty name and let the backend pick a drawable tier.
    portrait = await enqueue(() => fetchPortrait(store, props.model));
  } catch {
    return; // Not in this install; the initial stays.
  }
  if (!host.value) return; // unmounted while awaiting
  const { geo, urls } = portrait;

  teardown();

  const w = el.clientWidth || 84;
  const h = el.clientHeight || 104;

  const r = new THREE.WebGLRenderer({ antialias: true, alpha: true });
  r.setPixelRatio(Math.min(window.devicePixelRatio, 2));
  r.setSize(w, h, false);
  el.appendChild(r.domElement);
  renderer.value = r;

  const scene = new THREE.Scene();

  // Left-handed game space → right-handed three.js, the same way ModelViewer
  // does it: negate Z on positions and normals, then flip triangle winding to
  // cancel the orientation change. A negative mesh scale would reverse winding
  // and render the model inside-out.
  const positions = Float32Array.from(geo.positions);
  const normals = Float32Array.from(geo.normals);
  for (let i = 2; i < positions.length; i += 3) positions[i] = -positions[i];
  for (let i = 2; i < normals.length; i += 3) normals[i] = -normals[i];

  const indices = Uint32Array.from(geo.indices);
  for (let i = 0; i + 2 < indices.length; i += 3) {
    const t = indices[i + 1];
    indices[i + 1] = indices[i + 2];
    indices[i + 2] = t;
  }

  const g = new THREE.BufferGeometry();
  g.setAttribute("position", new THREE.BufferAttribute(positions, 3));
  g.setAttribute("normal", new THREE.BufferAttribute(normals, 3));
  g.setAttribute("uv", new THREE.BufferAttribute(Float32Array.from(geo.uvs), 2));
  g.setIndex(new THREE.BufferAttribute(indices, 1));

  // Upload the decoded skins. These are local data URLs by now, so this is a
  // GPU upload rather than any more WAD work.
  const maps = new Map<string, THREE.Texture>();
  const loader = new THREE.TextureLoader();
  await Promise.all(
    [...urls].map(
      ([name, url]) =>
        new Promise<void>((resolve) => {
          loader.load(
            url,
            (t) => {
              t.colorSpace = THREE.SRGBColorSpace;
              t.flipY = false; // the game's UVs already have the origin at the top
              maps.set(name, t);
              resolve();
            },
            undefined,
            () => resolve(),
          );
        }),
    ),
  );
  if (!host.value) return; // unmounted while uploading
  textures.value = [...maps.values()];

  // One material per group, so each part gets the right map. Groups without a
  // recoverable texture keep the paper tone rather than rendering black.
  const materials = geo.groups.map((grp) => {
    const d = grp.textures.find((t) => t.slot === "diffuse" && t.name);
    const map = d?.name ? (maps.get(d.name) ?? null) : null;
    return new THREE.MeshStandardMaterial({
      color: map ? 0xffffff : 0x9b9a80,
      map,
      roughness: 0.9,
      metalness: 0,
      side: THREE.FrontSide,
    });
  });
  geo.groups.forEach((grp, i) => g.addGroup(grp.index_start, grp.index_count, i));

  const mesh = new THREE.Mesh(g, materials);
  scene.add(mesh);

  // Bounding box in the flipped space — Z negated and ends swapped.
  const min = new THREE.Vector3(geo.bbox_min[0], geo.bbox_min[1], -geo.bbox_max[2]);
  const max = new THREE.Vector3(geo.bbox_max[0], geo.bbox_max[1], -geo.bbox_min[2]);
  const height = Math.max(max.y - min.y, 0.001);

  // Frame head-and-shoulders rather than the whole body: a full figure at this
  // size is an unreadable smudge, and a bust is what a note actually carries.
  const target = new THREE.Vector3(
    (min.x + max.x) * 0.5,
    min.y + height * EYE_LINE,
    (min.z + max.z) * 0.5,
  );
  const frame = height * FRAME;

  const fov = 32;
  const camera = new THREE.PerspectiveCamera(fov, w / h, height / 500, height * 20);
  const dist = (frame / Math.tan((fov * Math.PI) / 360)) * 1.25;
  // Three-quarter view. Characters face +Z once flipped, so the camera sits on
  // the -Z side; the X offset gives the angled pose an engraved portrait uses.
  camera.position.set(
    target.x + dist * 0.5,
    target.y + dist * 0.12,
    target.z - dist * 0.92,
  );
  camera.lookAt(target);

  // Strong key from the front-left with a soft fill, which is what gives the
  // cheekbone/jaw modelling that reads as engraved hatching at this size.
  scene.add(new THREE.AmbientLight(0xffffff, 1.15));
  const key = new THREE.DirectionalLight(0xfff6e0, 2.2);
  key.position.set(target.x + dist, target.y + dist * 0.7, target.z - dist);
  scene.add(key);
  const rim = new THREE.DirectionalLight(0x8b886e, 1.1);
  rim.position.set(target.x - dist, target.y + dist * 0.3, target.z + dist * 0.6);
  scene.add(rim);

  r.render(scene, camera);
  ready.value = true;
}

onMounted(draw);
onBeforeUnmount(teardown);
// The picker mounts before the game folder resolves on a cold start.
watch(() => store.gamePath, (p) => { if (p && !ready.value) void draw(); });
</script>

<template>
  <span
    class="cartouche guilloche relative flex h-[104px] w-[84px] items-center justify-center bg-zinc-900 transition"
    :class="active ? 'ring-1 ring-emerald-400/70' : 'opacity-55 group-hover:opacity-90'"
  >
    <div ref="host" class="absolute inset-0" aria-hidden="true" />
    <span
      v-if="!ready"
      class="text-2xl font-bold italic"
      :class="active ? 'text-emerald-300' : 'text-zinc-500'"
    >
      {{ fallback }}
    </span>
  </span>
</template>
