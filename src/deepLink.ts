import type { RouteLocationRaw } from "vue-router";
import type { useProjectStore } from "./stores/project";
import type { DeepLink, MarketLink } from "./types";

type Store = ReturnType<typeof useProjectStore>;
type Push = (to: RouteLocationRaw) => Promise<unknown>;

/**
 * Act on a `mercs2-modkit://` link, cold start or live.
 *
 * - `ship` (the Workshop's "Send to Modkit"): stage the handed-off Shipment and jump to the
 *   build/export page so it's in front of the user. Errors surface via `store.error`.
 * - `install` (mercs.ink's "Install with Modkit") and `invalid`: open the Mod Market and hand it
 *   the link in `store.pendingMarketLink`. The Mod Market installs it exactly as its own Install
 *   button does, or shows why the URL was refused. It, not this function, sets the message: it
 *   is the view that shows `store.error`, and it clears `store.error` as it loads.
 */
export async function handleDeepLink(link: DeepLink, store: Store, push: Push): Promise<void> {
  if (link.kind === "ship") {
    try {
      await store.importShipment(link.path);
      await push({ name: "export" });
    } catch {
      /* store.error is shown in the shell */
    }
    return;
  }
  // Navigate first: every navigation clears `store.error`.
  await push({ name: "catalog" });
  store.pendingMarketLink = link;
}

/**
 * The Mod Market's side: take the link handed over in `store.pendingMarketLink`, if any. An
 * `install` link goes to `install` (the Install button's own path; `version` absent is the
 * latest); a refused URL becomes `store.error`, naming it and what was wrong.
 */
export function takeMarketLink(
  store: Store,
  install: (slug: string, version?: string) => unknown,
): void {
  const link: MarketLink | null = store.pendingMarketLink;
  if (!link) return;
  store.pendingMarketLink = null;
  if (link.kind === "install") install(link.slug, link.version ?? undefined);
  else store.error = `Modkit can't open ${link.url}: ${link.reason}.`;
}
