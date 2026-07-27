<script setup lang="ts">
/**
 * Modkit's mark: a spade with the mohawk skull knocked out of it, after the
 * emblem in the game's own wordmark.
 *
 * Drawn rather than extracted. The game ships two versions of this art and
 * neither is usable here: `mercs2.ico` carries the whole wordmark at 60×18 of
 * real pixels, which is far too soft to enlarge, and the crisp one lives inside
 * `data/Movies/title_logo.bik` as Bink video with no decoder on this side. A
 * path also scales to any size, themes with the palette, and keeps EA's artwork
 * out of the repo — the reason `/storage/` is gitignored.
 *
 * One path, `fill-rule="evenodd"`: spade fills, skull knocks out, eye sockets
 * and nose fill back in. That nesting is what makes the mark work on any
 * background instead of only on the one colour it was drawn against.
 */
withDefaults(defineProps<{ title?: string }>(), { title: "Mercenaries 2 Modkit" });
</script>

<template>
  <svg
    viewBox="0 0 100 100"
    role="img"
    :aria-label="title"
    fill="currentColor"
    fill-rule="evenodd"
  >
    <path
      d="M50 4C50 4 12 37 12 56.5C12 70.5 22 78 33 78C39.5 78 45 74 47.5 68.5C46.5 80.5 41 90 32 96L68 96C59 90 53.5 80.5 52.5 68.5C55 74 60.5 78 67 78C78 78 88 70.5 88 56.5C88 37 50 4 50 4Z
         M45 13L50.5 25L54.5 17.5L57.5 28L62 24C69.5 30 74 39.5 74 50C74 57.5 71 64 66.5 68.5L66.5 79L62.5 83L58.5 79L54.5 83L50.5 79L46.5 83L42.5 79L38.5 83L34.5 79L34.5 68.5C30 64 27 57.5 27 50C27 39 32 29 40 23.5Z
         M33.5 44.5L46 50L42.5 58.5L31 54.5Z
         M67.5 44.5L55 50L58.5 58.5L70 54.5Z
         M50.5 55L54.5 65.5L46.5 65.5Z"
    />
  </svg>
</template>
