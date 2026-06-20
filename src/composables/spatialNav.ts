// Geometric ("spatial") focus navigation. Given the currently-focused element
// and a direction, find the nearest visible focusable element that lies that
// way on screen. Works against the app's native <button>/<a>/<input> controls,
// so no per-view tabindex wiring is needed.

export type Dir = "up" | "down" | "left" | "right";

const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

export function focusableElements(): HTMLElement[] {
  const nodes = Array.from(
    document.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)
  );
  return nodes.filter((el) => {
    // Skip hidden / zero-size elements. offsetParent is null for
    // display:none and for elements inside it (fixed-position is the
    // exception, which dialogs/overlays use).
    if (el.offsetParent === null && getComputedStyle(el).position !== "fixed") {
      return false;
    }
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  });
}

function center(r: DOMRect) {
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
}

// Lower score = better candidate: distance travelled in the pressed direction
// plus a heavy penalty for drifting off the cross-axis (keeps movement aligned).
export function findInDirection(
  current: HTMLElement | null,
  dir: Dir
): HTMLElement | null {
  const els = focusableElements();
  if (els.length === 0) return null;
  if (!current || !els.includes(current)) return els[0];

  const from = center(current.getBoundingClientRect());
  let best: HTMLElement | null = null;
  let bestScore = Infinity;

  for (const el of els) {
    if (el === current) continue;
    const c = center(el.getBoundingClientRect());
    const dx = c.x - from.x;
    const dy = c.y - from.y;

    let primary: number;
    let cross: number;
    switch (dir) {
      case "up":
        if (dy >= -1) continue;
        primary = -dy;
        cross = Math.abs(dx);
        break;
      case "down":
        if (dy <= 1) continue;
        primary = dy;
        cross = Math.abs(dx);
        break;
      case "left":
        if (dx >= -1) continue;
        primary = -dx;
        cross = Math.abs(dy);
        break;
      case "right":
        if (dx <= 1) continue;
        primary = dx;
        cross = Math.abs(dy);
        break;
    }

    const score = primary + cross * 2;
    if (score < bestScore) {
      bestScore = score;
      best = el;
    }
  }
  return best;
}
