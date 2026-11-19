import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import type { RouteLocationRaw } from "vue-router";
import { useProjectStore } from "./stores/project";
import { handleDeepLink, takeMarketLink } from "./deepLink";
import type { DeepLink, MercsInkInstall, ShipmentRef } from "./types";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

function shipment(id: string): ShipmentRef {
  return {
    id,
    name: id,
    path: `/staging/${id}`,
    slug: id,
    version: "1.2.0",
    origin: { source: "registry", id: `${id}-1`, version: "1.2.0" },
    install_reason: "user",
  };
}

function installed(id: string): MercsInkInstall {
  return {
    shipment: shipment(id),
    slug: id,
    title: null,
    release_version: "1.2.0",
    target: null,
    assets: [],
    staged_files: 1,
    dependencies: [],
  };
}

/**
 * A router stand-in that records where it was sent and, like the app's `router.afterEach`, clears
 * `store.error` on every navigation. `pendingAtPush` records what the Mod Market had been handed
 * when the navigation happened.
 */
function fakeRouter(store: ReturnType<typeof useProjectStore>) {
  const pushed: RouteLocationRaw[] = [];
  const pendingAtPush: unknown[] = [];
  const push = async (to: RouteLocationRaw) => {
    pushed.push(to);
    pendingAtPush.push(store.pendingMarketLink);
    store.error = null;
  };
  return { push, pushed, pendingAtPush };
}

describe("Deep links", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("an install link opens the Mod Market, then hands it the release", async () => {
    const store = useProjectStore();
    const router = fakeRouter(store);
    const link: DeepLink = { kind: "install", slug: "vehicle-pack", version: "1.2.0" };

    await handleDeepLink(link, store, router.push);

    expect(router.pushed).toEqual([{ name: "catalog" }]);
    expect(router.pendingAtPush).toEqual([null]);
    expect(store.pendingMarketLink).toEqual(link);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("the Mod Market installs a pinned release through installFromRegistry", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "install_mercsink_shipment") return installed("vehicle-pack");
      throw new Error(`unexpected command in this test: ${cmd}`);
    });
    const store = useProjectStore();
    store.gamePath = "/games/mercs2";
    store.pendingMarketLink = { kind: "install", slug: "vehicle-pack", version: "1.2.0" };

    const install = vi.fn((slug: string, version?: string) => store.installFromRegistry(slug, version));
    takeMarketLink(store, install);
    await install.mock.results[0].value;

    expect(install).toHaveBeenCalledWith("vehicle-pack", "1.2.0");
    expect(store.pendingMarketLink).toBeNull();
    expect(invokeMock).toHaveBeenCalledWith("install_mercsink_shipment", {
      slug: "vehicle-pack",
      version: "1.2.0",
      installed: [],
      gamePath: "/games/mercs2",
    });
    expect(store.shipments.map((s) => s.id)).toEqual(["vehicle-pack"]);
    expect(store.error).toBeNull();
  });

  it("an install link without a version installs the latest", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "install_mercsink_shipment") return installed("vehicle-pack");
      throw new Error(`unexpected command in this test: ${cmd}`);
    });
    const store = useProjectStore();
    store.pendingMarketLink = { kind: "install", slug: "vehicle-pack", version: null };

    const install = vi.fn((slug: string, version?: string) => store.installFromRegistry(slug, version));
    takeMarketLink(store, install);
    await install.mock.results[0].value;

    expect(install).toHaveBeenCalledWith("vehicle-pack", undefined);
    expect(invokeMock).toHaveBeenCalledWith(
      "install_mercsink_shipment",
      expect.objectContaining({ slug: "vehicle-pack", version: null }),
    );
  });

  it("an install the backend refuses leaves its message in store.error", async () => {
    invokeMock.mockRejectedValueOnce("Set the game folder before building Shipments.");
    const store = useProjectStore();
    store.shipments = [shipment("ess")];
    store.pendingMarketLink = { kind: "install", slug: "vehicle-pack", version: null };

    const install = vi.fn((slug: string, version?: string) => store.installFromRegistry(slug, version));
    takeMarketLink(store, install);

    await expect(install.mock.results[0].value).rejects.toBe(
      "Set the game folder before building Shipments.",
    );
    expect(store.error).toBe("Set the game folder before building Shipments.");
  });

  it("a refused URL opens the Mod Market, which names the URL and what was wrong", async () => {
    const store = useProjectStore();
    const router = fakeRouter(store);
    const link: DeepLink = {
      kind: "invalid",
      url: "mercs2-modkit://install?version=1.0.0",
      reason: "it has no mod to install",
    };

    await handleDeepLink(link, store, router.push);
    expect(router.pushed).toEqual([{ name: "catalog" }]);

    const install = vi.fn();
    takeMarketLink(store, install);

    expect(install).not.toHaveBeenCalled();
    expect(store.pendingMarketLink).toBeNull();
    expect(store.error).toBe(
      "Modkit can't open mercs2-modkit://install?version=1.0.0: it has no mod to install.",
    );
  });

  it("the Mod Market does nothing when no link was handed over", () => {
    const store = useProjectStore();
    const install = vi.fn();
    takeMarketLink(store, install);
    expect(install).not.toHaveBeenCalled();
    expect(store.error).toBeNull();
  });

  it("a ship link stages the Shipment and opens Build & Deploy", async () => {
    const staged = shipment("my-mod");
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "inspect_shipment") return staged;
      if (cmd === "plan_mercsink_dependencies") return { unmet: [], install: [], unresolved: null };
      throw new Error(`unexpected command in this test: ${cmd}`);
    });
    const store = useProjectStore();
    const router = fakeRouter(store);

    await handleDeepLink({ kind: "ship", path: staged.path }, store, router.push);

    expect(invokeMock).toHaveBeenCalledWith("inspect_shipment", { path: staged.path });
    expect(store.shipments.map((s) => s.id)).toEqual(["my-mod"]);
    expect(router.pushed).toEqual([{ name: "export" }]);
    expect(store.pendingMarketLink).toBeNull();
  });
});
