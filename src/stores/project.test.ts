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
