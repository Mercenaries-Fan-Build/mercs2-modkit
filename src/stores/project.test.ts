import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { useProjectStore } from "./project";
import type { BuildResult, IncompatibilityCheck, ShipmentRef, ShipmentRemovalPlan } from "../types";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

function shipment(id: string, install_reason: ShipmentRef["install_reason"]): ShipmentRef {
  return {
    id,
    name: id,
    path: `/staging/${id}`,
    slug: id,
    version: "1.0.0",
    origin: { source: "registry", id: `${id}-1`, version: "1.0.0" },
    install_reason,
  };
}

describe("Shipment removal", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("cancelling a planned removal never removes anything", async () => {
    const consumer = shipment("vehicle-pack", "user");
    const ess = shipment("ess", "dependency");
    const bridge = shipment("lua-bridge", "dependency");
    const plan: ShipmentRemovalPlan = {
      removed: consumer,
      missing_too: [],
      cascade: [],
      orphans: [ess, bridge],
    };
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "plan_shipment_removal") return plan;
      throw new Error(`unexpected command in this test: ${cmd}`);
    });

    const store = useProjectStore();
    store.shipments = [consumer, ess, bridge];
    // A deep copy taken outside the store's reactive proxy.
    const before: ShipmentRef[] = JSON.parse(JSON.stringify(store.shipments));

    await store.planShipmentRemoval(consumer.id);
    expect(store.error).toBeNull();
    expect(store.pendingShipmentRemoval).toEqual(plan);

    store.cancelShipmentRemoval();

    expect(store.pendingShipmentRemoval).toBeNull();
    expect(invokeMock.mock.calls.map(([cmd]) => cmd)).toEqual(["plan_shipment_removal"]);
    expect(invokeMock).not.toHaveBeenCalledWith("remove_shipments", expect.anything());
    expect(store.shipments).toEqual(before);
  });
});

describe("Shipments whose folders are missing", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("a reinstall replaces the broken rows in place, even where the Shipment root moved", async () => {
    const sdk = shipment("m2-sdk", "dependency");
    const patch = shipment("patch", "user");
    const ui = shipment("ui", "user");
    const sdkBack = { ...sdk, path: "/staging/m2-sdk/m2-sdk-v2" };
    const patchBack = { ...patch, version: "2.0.0" };
    let missing = [sdk.id, patch.id];
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "reinstall_missing_shipment") {
        missing = [];
        return { shipment: patchBack, dependencies: [{ shipment: sdkBack }] };
      }
      if (cmd === "missing_shipment_folders") return missing;
      throw new Error(`unexpected command in this test: ${cmd}`);
    });

    const store = useProjectStore();
    store.shipments = [sdk, patch, ui];
    await store.refreshMissingShipments();
    expect(store.missingShipmentIds).toEqual([sdk.id, patch.id]);

    await store.reinstallMissingShipment(patch.id);
    expect(store.error).toBeNull();
    expect(store.shipments.map((s) => [s.id, s.path, s.version])).toEqual([
      [sdk.id, sdkBack.path, "1.0.0"],
      [patch.id, patch.path, "2.0.0"],
      [ui.id, ui.path, "1.0.0"],
    ]);
    expect(store.missingShipmentIds).toEqual([]);
  });

  it("confirming removes the chosen row and every other missing row with it", async () => {
    const sdk = shipment("m2-sdk", "dependency");
    const patch = shipment("patch", "user");
    const ui = shipment("ui", "user");
    const keep = shipment("keep", "user");
    const plan: ShipmentRemovalPlan = {
      removed: patch,
      missing_too: [sdk],
      cascade: [{ shipment: ui, pulled_by: { kind: "shipment", target: "m2-sdk", range: null } }],
      orphans: [],
    };
    let removedRows: string[] = [];
    invokeMock.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "plan_shipment_removal") return plan;
      if (cmd === "remove_shipments") {
        removedRows = (args as { rows: ShipmentRef[] }).rows.map((r) => r.id);
        return { removed: removedRows, failed: null, not_attempted: [] };
      }
      if (cmd === "missing_shipment_folders") return [];
      throw new Error(`unexpected command in this test: ${cmd}`);
    });

    const store = useProjectStore();
    store.shipments = [sdk, patch, ui, keep];
    await store.planShipmentRemoval(patch.id);
    await store.confirmShipmentRemoval();
    expect(removedRows).toEqual([patch.id, sdk.id, ui.id]);
    expect(store.shipments.map((s) => s.id)).toEqual([keep.id]);
  });
});

describe("Incompatibility check", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  const check: IncompatibilityCheck = {
    list: {
      state: "cached",
      fetched_at: 1_000,
      generated_at: "2026-09-25T10:00:00+00:00",
      reason: "it answered HTTP 503",
      message: "Couldn't check mercs.ink for incompatibility reports (it answered HTTP 503).",
    },
    notices: [],
  };

  function built(incompatibilities: IncompatibilityCheck | null): BuildResult {
    return {
      path: "",
      staging_dir: "/build",
      block_count: 0,
      byte_size: 0,
      sha256: "",
      outcomes: [],
      stream_copies: [],
      data_files: [],
      incompatibilities,
    };
  }

  it("is set from a build and cleared by the next build, even one that fails", async () => {
    const store = useProjectStore();
    invokeMock.mockResolvedValueOnce(built(check));
    await store.assemble();
    expect(store.incompatibilityCheck).toEqual(check);

    invokeMock.mockRejectedValueOnce("mercs.ink lists a confirmed incompatibility");
    await expect(store.assemble()).rejects.toBe("mercs.ink lists a confirmed incompatibility");
    expect(store.incompatibilityCheck).toBeNull();
    expect(invokeMock.mock.calls.map(([cmd]) => cmd)).toEqual(["assemble_patch_wad", "assemble_patch_wad"]);
  });
});
