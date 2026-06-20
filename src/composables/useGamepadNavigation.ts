// Wires raw gamepad actions to the app: directional input moves DOM focus
// spatially, A activates the focused control, B goes back / closes a dialog,
// the bumpers cycle the sidebar, the right stick scrolls, and Start plays.
// Call once, from App.vue's setup.

import { ref } from "vue";
import { useRouter } from "vue-router";
import { useGamepad, type GamepadAction } from "./useGamepad";
import { findInDirection, focusableElements, type Dir } from "./spatialNav";

const SCROLL_SPEED = 18; // px per frame at full right-stick deflection

export function useGamepadNavigation() {
  const router = useRouter();

  // Reactive connection state, plus the controller's reported name.
  const connected = ref(false);
  const controllerId = ref<string | null>(null);

  function setGamepadMode(on: boolean) {
    document.body.classList.toggle("gamepad-active", on);
  }

  function focusEl(el: HTMLElement | null) {
    if (!el) return;
    el.focus();
    el.scrollIntoView({ block: "nearest", inline: "nearest" });
  }

  function move(dir: Dir) {
    const active = document.activeElement as HTMLElement | null;
    focusEl(findInDirection(active, dir));
  }

  // A dialog (HeadlessUI) renders role="dialog" and traps focus while open.
  function openDialog(): HTMLElement | null {
    return document.querySelector<HTMLElement>('[role="dialog"]');
  }

  function confirm() {
    const el = document.activeElement as HTMLElement | null;
    if (el && focusableElements().includes(el)) el.click();
    else focusEl(focusableElements()[0] ?? null);
  }

  function back() {
    // If a modal is open, let its own Escape handler close it; otherwise
    // walk back through the router history.
    if (openDialog()) {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true })
      );
      return;
    }
    router.back();
  }

  // Cycle the sidebar sections. The links are plain <a> with .nav-link;
  // the active one carries .nav-link-active. Querying live DOM means the
  // conditional "Setup" link is handled automatically.
  function cycleNav(delta: 1 | -1) {
    const links = Array.from(
      document.querySelectorAll<HTMLAnchorElement>(".nav-link")
    );
    if (links.length === 0) return;
    const activeIdx = links.findIndex((l) =>
      l.classList.contains("nav-link-active")
    );
    const next = links[(activeIdx + delta + links.length) % links.length];
    next?.click();
  }

  function play() {
    // GameBar tags its Play/Stop button so the controller can reach it
    // regardless of which view is showing.
    const btn = document.querySelector<HTMLElement>("[data-gamepad-play]");
    btn?.click();
  }

  function scroll(dy: number) {
    const main = document.querySelector<HTMLElement>("main");
    main?.scrollBy({ top: dy * SCROLL_SPEED });
  }

  const actions: Record<GamepadAction, () => void> = {
    up: () => move("up"),
    down: () => move("down"),
    left: () => move("left"),
    right: () => move("right"),
    confirm,
    back,
    tabPrev: () => cycleNav(-1),
    tabNext: () => cycleNav(1),
    play,
  };

  useGamepad({
    onAction: (a) => {
      setGamepadMode(true);
      actions[a]();
    },
    onScroll: scroll,
    onConnect: (id) => {
      connected.value = true;
      controllerId.value = id;
      setGamepadMode(true);
      // Seed focus so there's a visible starting point.
      if (!focusableElements().includes(document.activeElement as HTMLElement)) {
        focusEl(focusableElements()[0] ?? null);
      }
    },
    onDisconnect: () => {
      connected.value = false;
      controllerId.value = null;
      setGamepadMode(false);
    },
  });

  return { connected, controllerId };
}
