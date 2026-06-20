// Reads a Standard-mapping gamepad (Xbox / PlayStation / most others) through
// the browser Gamepad API, which runs inside the Tauri webview. The API is
// poll-based, so we diff button state every animation frame and surface clean,
// semantic actions. Directional inputs auto-repeat while held; everything else
// fires once per press. No-op when no controller is present.

import { onMounted, onBeforeUnmount } from "vue";

export type GamepadAction =
  | "up"
  | "down"
  | "left"
  | "right"
  | "confirm"
  | "back"
  | "tabPrev"
  | "tabNext"
  | "play";

export interface GamepadHandlers {
  onAction?: (action: GamepadAction) => void;
  onScroll?: (dy: number) => void; // right-stick Y, normalized -1..1
  onConnect?: (id: string) => void;
  onDisconnect?: () => void;
}

const STICK_DEADZONE = 0.5; // left stick must pass this to count as a direction
const SCROLL_DEADZONE = 0.15; // right stick must pass this to scroll
const REPEAT_DELAY = 400; // ms a direction is held before it auto-repeats
const REPEAT_INTERVAL = 110; // ms between auto-repeats

// Standard Gamepad mapping button indices.
const BTN = {
  confirm: 0, // A / Cross
  back: 1, // B / Circle
  tabPrev: 4, // LB / L1
  tabNext: 5, // RB / R1
  play: 9, // Start / Options
  dUp: 12,
  dDown: 13,
  dLeft: 14,
  dRight: 15,
} as const;

type Dir = "up" | "down" | "left" | "right";

export function useGamepad(handlers: GamepadHandlers) {
  let raf = 0;
  let haveConnected = false;

  // Rising-edge tracking for one-shot buttons, keyed by button index.
  const prevPressed: Record<number, boolean> = {};
  // Auto-repeat bookkeeping for the four directions.
  const wasActive: Record<Dir, boolean> = {
    up: false,
    down: false,
    left: false,
    right: false,
  };
  const nextFire: Record<Dir, number> = { up: 0, down: 0, left: 0, right: 0 };

  function activePad(): Gamepad | null {
    if (!navigator.getGamepads) return null;
    const pads = navigator.getGamepads();
    // Prefer a standard-mapped pad; fall back to any connected one.
    for (const p of pads) if (p && p.connected && p.mapping === "standard") return p;
    for (const p of pads) if (p && p.connected) return p;
    return null;
  }

  function isPressed(p: Gamepad, i: number): boolean {
    const b = p.buttons[i];
    return !!b && (b.pressed || b.value > 0.5);
  }

  function edge(p: Gamepad, i: number): boolean {
    const now = isPressed(p, i);
    const fired = now && !prevPressed[i];
    prevPressed[i] = now;
    return fired;
  }

  function dirActive(p: Gamepad, dir: Dir): boolean {
    const ax = p.axes;
    switch (dir) {
      case "up":
        return isPressed(p, BTN.dUp) || (ax[1] ?? 0) < -STICK_DEADZONE;
      case "down":
        return isPressed(p, BTN.dDown) || (ax[1] ?? 0) > STICK_DEADZONE;
      case "left":
        return isPressed(p, BTN.dLeft) || (ax[0] ?? 0) < -STICK_DEADZONE;
      case "right":
        return isPressed(p, BTN.dRight) || (ax[0] ?? 0) > STICK_DEADZONE;
    }
  }

  function pumpDir(p: Gamepad, dir: Dir, t: number) {
    const active = dirActive(p, dir);
    if (active && !wasActive[dir]) {
      handlers.onAction?.(dir);
      nextFire[dir] = t + REPEAT_DELAY;
    } else if (active && t >= nextFire[dir]) {
      handlers.onAction?.(dir);
      nextFire[dir] = t + REPEAT_INTERVAL;
    }
    wasActive[dir] = active;
  }

  function loop(t: number) {
    const p = activePad();
    if (p) {
      if (!haveConnected) {
        haveConnected = true;
        handlers.onConnect?.(p.id);
      }
      (["up", "down", "left", "right"] as Dir[]).forEach((d) => pumpDir(p, d, t));
      if (edge(p, BTN.confirm)) handlers.onAction?.("confirm");
      if (edge(p, BTN.back)) handlers.onAction?.("back");
      if (edge(p, BTN.tabPrev)) handlers.onAction?.("tabPrev");
      if (edge(p, BTN.tabNext)) handlers.onAction?.("tabNext");
      if (edge(p, BTN.play)) handlers.onAction?.("play");

      const sy = p.axes[3] ?? 0; // right stick Y
      if (Math.abs(sy) > SCROLL_DEADZONE) handlers.onScroll?.(sy);
    }
    raf = requestAnimationFrame(loop);
  }

  function onConnected(e: GamepadEvent) {
    haveConnected = true;
    handlers.onConnect?.(e.gamepad.id);
  }
  function onDisconnected() {
    if (!activePad()) {
      haveConnected = false;
      handlers.onDisconnect?.();
    }
  }

  onMounted(() => {
    window.addEventListener("gamepadconnected", onConnected);
    window.addEventListener("gamepaddisconnected", onDisconnected);
    raf = requestAnimationFrame(loop);
  });
  onBeforeUnmount(() => {
    window.removeEventListener("gamepadconnected", onConnected);
    window.removeEventListener("gamepaddisconnected", onDisconnected);
    cancelAnimationFrame(raf);
  });
}
