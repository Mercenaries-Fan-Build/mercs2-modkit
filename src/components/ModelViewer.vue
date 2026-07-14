<script setup lang="ts">
/**
 * Show a model in 3D with the parts that use a given texture lit up.
 *
 * A list of model names tells you *that* a texture is used. This tells you **where** — is
 * `pmc_hum_chris_ub` the torso or the arms? The answer is exact, not inferred: the geometry
 * decoder splits a model into draw groups and each one records the texture hashes its
 * material samples, so the backend can flag precisely the groups that bind this texture.
 * Here we just make those glow and mute everything else.
 */
import { ref, shallowRef, watch, onBeforeUnmount, onMounted } from "vue";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { useProjectStore } from "../stores/project";
import Spinner from "./Spinner.vue";
import type { ModelGeometry } from "../types";

const props = defineProps<{
  /** Model to draw. */
  model: string;
  /** Texture to highlight within it. */
  texture: string;
  /** Optional decoded texture image (data URL) to paint on the highlighted parts. */
  textureUrl?: string | null;
  /**
   * Force a specific SEGM state bit. Omit and the backend picks one that shows the texture —
   * which matters, because a texture can be painted in one state and absent in another.
   */
  tier?: number | null;
  /** Isolate one part (a draw group id): it lights up, everything else fades right back. */
  selected?: number | null;
}>();

const emit = defineEmits<{ (e: "loaded", geo: ModelGeometry): void }>();

const store = useProjectStore();

const host = ref<HTMLDivElement | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
const info = ref<{ tris: number; groups: number; highlighted: number } | null>(null);

// three.js objects are large and deeply nested; keeping them out of Vue's reactive proxy
// avoids a lot of pointless proxying (and some genuinely weird bugs).
const three = shallowRef<{
  renderer: THREE.WebGLRenderer;
  scene: THREE.Scene;
  camera: THREE.PerspectiveCamera;
  controls: OrbitControls;
  raf: number;
  ro: ResizeObserver;
  /** One material per draw group, index-aligned with `geo.groups`. */
  materials: THREE.MeshStandardMaterial[];
  geo: ModelGeometry;
} | null>(null);

const HIGHLIGHT = 0x10b981; // emerald — matches the app's accent
const ISOLATED = 0x38bdf8; // sky — a *selected* part, distinct from "uses this texture"
const MUTED = 0x71717a; // zinc-500: light enough to read the shape under the bright rig

/**
 * Style each part according to what the user is looking at. Called on load and whenever the
 * selection changes — no scene rebuild, so isolating a part is instant.
 *
 * Two different questions get two different colours, deliberately:
 *  - emerald = "this part uses the texture" (the page's subject)
 *  - sky     = "this is the part you selected" (which may or may not use it)
 */
function style() {
  const t = three.value;
  if (!t) return;
  const sel = props.selected ?? null;

  t.geo.groups.forEach((grp, i) => {
    const m = t.materials[i];
    const isSel = sel !== null && grp.id === sel;

    if (isSel) {
      // The selected part, lit. Green if it wears this texture, sky if it wears another —
      // "what you picked" and "what uses the texture" are different questions.
      m.color.setHex(grp.uses_texture ? 0x86efac : 0xbae6fd);
      m.emissive.setHex(grp.uses_texture ? HIGHLIGHT : ISOLATED);
      m.emissiveIntensity = 0.8;
      m.opacity = 1;
    } else if (sel !== null) {
      // Something ELSE is isolated: every other part becomes the same neutral ghost — even
      // the ones using the texture. Merely fading them doesn't work: the parts are DoubleSide
      // and overlap (a shirt's front and back, arms behind it), so several translucent green
      // layers re-accumulate to near-opaque and keep shouting over your selection. Draining
      // the colour is what actually makes the isolation read.
      m.color.setHex(MUTED);
      m.emissive.setHex(0x000000);
      m.emissiveIntensity = 0;
      m.opacity = 0.1;
    } else if (grp.uses_texture) {
      // A green base, not white: under the bright rig a white base blows out and green
      // emissive on top of it reads as "slightly warm white" — i.e. as nothing.
      m.color.setHex(0x86efac);
      m.emissive.setHex(HIGHLIGHT);
      m.emissiveIntensity = 0.8;
      m.opacity = 1;
    } else {
      // Solid, not translucent. A see-through body lets you look straight through the torso
      // at its own far side, which reads as the model being inside-out. Transparency is for
      // ghosting when a part is isolated — not for the default view.
      m.color.setHex(MUTED);
      m.emissive.setHex(0x000000);
      m.emissiveIntensity = 0;
      m.opacity = 1;
    }
    m.depthWrite = m.opacity > 0.9;
    m.needsUpdate = true;
  });
}

function disposeScene() {
  const t = three.value;
  if (!t) return;
  cancelAnimationFrame(t.raf);
  t.ro.disconnect();
  t.controls.dispose();
  t.scene.traverse((o) => {
    const m = o as THREE.Mesh;
    m.geometry?.dispose?.();
    const mat = m.material;
    if (Array.isArray(mat)) mat.forEach((x) => x.dispose());
    else mat?.dispose?.();
  });
  t.renderer.dispose();
  t.renderer.domElement.remove();
  three.value = null;
}

function build(geo: ModelGeometry) {
  disposeScene();
  const el = host.value;
  if (!el) return;

  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
  renderer.setSize(el.clientWidth, el.clientHeight);
  el.appendChild(renderer.domElement);

  const scene = new THREE.Scene();

  // ── Left-handed game space → right-handed three.js space ──
  //
  // The obvious move is `mesh.scale.z = -1`. Don't: a negative scale REVERSES triangle
  // winding, so every face ends up back-to-front. Setting `DoubleSide` hides the error by
  // drawing the back faces as well — which is exactly why the model looked inside-out, its
  // interior showing through.
  //
  // Do it properly instead: negate Z on the positions and normals, then flip the winding of
  // every triangle to cancel the orientation change the negation caused. The result is a
  // correctly-wound right-handed mesh that can be rendered with backface culling, the way
  // the game itself draws it.
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
  g.setAttribute("uv", new THREE.Float32BufferAttribute(geo.uvs, 2));
  g.setIndex(new THREE.BufferAttribute(indices, 1));

  // One three.js group per draw group, so each part can take its own material — and so a
  // single part can be isolated without rebuilding anything.
  const materials: THREE.MeshStandardMaterial[] = geo.groups.map((grp, i) => {
    g.addGroup(grp.index_start, grp.index_count, i);
    return new THREE.MeshStandardMaterial({
      roughness: 0.8,
      metalness: 0.0,
      // Cull backfaces, like the game does. Now that the winding is right this is safe, and
      // it's what stops you seeing the inside of the torso through the front of it.
      side: THREE.FrontSide,
      transparent: true,
    });
  });

  // No mesh.scale: the handedness flip is baked into the vertex data above, correctly.
  const mesh = new THREE.Mesh(g, materials);
  scene.add(mesh);

  // Frame the model from its own bounding box — no magic camera distances. The box comes
  // from the engine's (left-handed) space, so its Z must be negated and its ends swapped to
  // match the geometry we just flipped.
  const min = new THREE.Vector3(geo.bbox_min[0], geo.bbox_min[1], -geo.bbox_max[2]);
  const max = new THREE.Vector3(geo.bbox_max[0], geo.bbox_max[1], -geo.bbox_min[2]);
  const center = min.clone().add(max).multiplyScalar(0.5);
  const radius = Math.max(max.clone().sub(min).length() * 0.5, 0.001);

  const camera = new THREE.PerspectiveCamera(
    45,
    el.clientWidth / Math.max(el.clientHeight, 1),
    radius / 100,
    radius * 100,
  );
  // Characters face -Z in game space, which the Z-flip above turns into +Z, so a camera on
  // the +Z side stares at the back of their head. Sit in front of them instead.
  camera.position.set(center.x + radius * 1.3, center.y + radius * 0.35, center.z - radius * 2.2);

  const controls = new OrbitControls(camera, renderer.domElement);
  controls.target.copy(center);
  controls.enableDamping = true;
  controls.update();

  // Lighting. Game models are authored for a bright outdoor scene and their diffuse maps are
  // dark, so a timid rig leaves them muddy. Light generously from every side — the point of
  // this view is to READ the shape, not to look moody.
  //
  // The ambient reaches surfaces no directional light does (inside an arm, under a chin), so
  // nothing on the model is ever pitch black. It is NOT cranked to the ceiling, though:
  // blowing the whole model out to white also washes the emerald highlight into white, and
  // then the one thing this view exists to show stops being visible.
  scene.add(new THREE.AmbientLight(0xffffff, 1.0));

  // Sky/ground hemisphere: cool from above, warm bounce from below. This alone stops the
  // undersides from going flat black the way a bare ambient would.
  scene.add(new THREE.HemisphereLight(0xffffff, 0x606070, 0.9));

  // Four-point directional rig, so the model is lit however you orbit it.
  const dirs: [number, number, number, number][] = [
    [2, 3, 3, 1.4], // key, front-high
    [-2.5, 1.5, 2, 0.8], // fill, opposite side
    [0, 2, -3, 0.7], // back/rim, keeps the silhouette off the background
    [0, -2, 1, 0.4], // bounce, from below
  ];
  for (const [x, y, z, intensity] of dirs) {
    const l = new THREE.DirectionalLight(0xffffff, intensity);
    l.position.set(x, y, z);
    scene.add(l);
  }

  // Slightly boost exposure — the muted material is deliberately dim, and the highlight
  // should still pop against it.
  renderer.toneMapping = THREE.NoToneMapping;

  // Paint the real texture onto the highlighted parts, if we have it decoded.
  if (props.textureUrl) {
    new THREE.TextureLoader().load(props.textureUrl, (map) => {
      map.colorSpace = THREE.SRGBColorSpace;
      map.flipY = false; // the game's UVs already have the origin at the top
      geo.groups.forEach((grp, i) => {
        if (!grp.uses_texture) return;
        materials[i].map = map;
        materials[i].needsUpdate = true;
      });
    });
  }

  const clock = new THREE.Clock();
  const state = {
    renderer,
    scene,
    camera,
    controls,
    raf: 0,
    ro: null as unknown as ResizeObserver,
    materials,
    geo,
  };

  const tick = () => {
    state.raf = requestAnimationFrame(tick);
    // A slow pulse on the emissive so the lit parts are unmissable without being a strobe.
    const pulse = 0.7 + 0.3 * Math.sin(clock.getElapsedTime() * 2.0);
    const sel = props.selected ?? null;
    geo.groups.forEach((grp, i) => {
      const lit = sel !== null ? grp.id === sel : grp.uses_texture;
      if (lit) materials[i].emissiveIntensity = pulse;
    });
    controls.update();
    renderer.render(scene, camera);
  };

  const ro = new ResizeObserver(() => {
    const w = el.clientWidth;
    const h = Math.max(el.clientHeight, 1);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
    renderer.setSize(w, h);
  });
  ro.observe(el);
  state.ro = ro;

  three.value = state;
  style();
  tick();

  info.value = {
    tris: geo.indices.length / 3,
    groups: geo.groups.length,
    highlighted: geo.highlighted_groups,
  };
  emit("loaded", geo);
}

// Selection only changes materials — never rebuild the scene for it.
watch(() => props.selected, style);

async function load() {
  loading.value = true;
  error.value = null;
  info.value = null;
  try {
    const geo = await store.modelGeometry(props.model, props.texture, props.tier ?? null);
    build(geo);
  } catch (e) {
    disposeScene();
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

onMounted(load);
watch(() => [props.model, props.texture, props.textureUrl, props.tier], load);
onBeforeUnmount(disposeScene);
</script>

<template>
  <div class="relative overflow-hidden rounded-xl border border-zinc-800 bg-black/40">
    <div ref="host" class="h-80 w-full" />

    <div
      v-if="loading"
      class="absolute inset-0 flex flex-col items-center justify-center bg-black/60 text-center"
    >
      <Spinner class="h-6 w-6" />
      <p class="mt-2 text-xs text-zinc-400">Loading the model…</p>
    </div>

    <p
      v-else-if="error"
      class="absolute inset-0 flex items-center justify-center px-6 text-center text-xs text-red-300"
    >
      {{ error }}
    </p>

    <template v-else-if="info">
      <div
        class="pointer-events-none absolute left-3 top-3 rounded-lg bg-black/60 px-2.5 py-1.5 text-[11px] text-zinc-400"
      >
        <span class="text-emerald-400">●</span>
        {{ info.highlighted }} of {{ info.groups }} parts use this texture
      </div>
      <div
        class="pointer-events-none absolute bottom-3 right-3 rounded-lg bg-black/60 px-2.5 py-1.5 text-[11px] text-zinc-500"
      >
        drag to rotate · scroll to zoom
      </div>
      <p
        v-if="info.highlighted === 0"
        class="pointer-events-none absolute inset-x-0 bottom-10 text-center text-[11px] text-amber-300/80"
      >
        Not painted on this version — try another state below.
      </p>
    </template>
  </div>
</template>
