import { describe, expect, it } from "vitest";
import { backoffAfterClose, reconnectDelay } from "./transport";
import type { RemoteSnapshot } from "./types";
import { cleanName, nameKey, parseEvent, parseTargetParse } from "./validate";

const snapshot: RemoteSnapshot = {
  pcName: "Gaming PC",
  appVersion: "1.3.0",
  role: "readonly",
  vaultLocked: false,
  windowsLocked: false,
  accounts: [{ id: 1, username: "a", displayName: "A", alias: "", color: null, main: true, status: "valid", groupIds: [2] }],
  groups: [{ id: 2, name: "Alts" }],
  favourites: [{ id: 3, name: "Fav", placeId: null }],
  instances: [
    {
      id: "i1",
      accountId: 1,
      state: "inGame",
      target: { kind: "place", placeId: 5 },
      placeId: 5,
      startedAt: 0,
      endedAt: null,
      note: null,
      limits: { cpuCap: null, eco: false, trimEveryMin: null, affinity: null },
      antiAfk: false,
      fpsCap: null,
      relaunching: false,
    },
  ],
  metrics: [{ instanceId: "i1", pid: 9, cpuPct: null, memoryMb: 1, privateMb: 1, readBps: null, writeBps: null, gpuPct: null, gpuMemMb: null }],
  systemUsage: { cpuPct: 3, memUsedMb: 1, memTotalMb: 2 },
  bulkProgress: { batchId: "b", done: false, items: [{ accountId: 1, userId: 7, stage: { kind: "failed", message: "x" }, instanceId: null }] },
  shutdownAt: null,
  crashes: [{ id: 1, accountId: 1, kind: "crash", at: 0, reason: null }],
};

describe("parseEvent", () => {
  it("accepts well-formed events", () => {
    expect(parseEvent({ t: "snapshot", batch: 1, data: snapshot })).not.toBeNull();
    expect(parseEvent({ t: "accountsPage", batch: 1, offset: 1, accounts: snapshot.accounts })).not.toBeNull();
    expect(parseEvent({ t: "page", batch: 1, offset: 1, page: { list: "instances", items: snapshot.instances } })).not.toBeNull();
    expect(parseEvent({ t: "page", batch: 1, offset: 1, page: { list: "bulkItems", items: snapshot.bulkProgress!.items } })).not.toBeNull();
    expect(parseEvent({ t: "rpcResult", reqId: 4, ok: true, data: { pong: true } })).not.toBeNull();
    expect(parseEvent({ t: "rpcResult", reqId: 4, ok: false, code: "rate_limited", error: "slow down" })).not.toBeNull();
  });

  it("drops events that would break rendering", () => {
    const bad: unknown[] = [
      null,
      [],
      { t: "nope" },
      { t: "snapshot", batch: 1, data: { pcName: "x", role: "control" } },
      { t: "snapshot", batch: 1, data: { ...snapshot, instances: null } },
      { t: "snapshot", batch: 1, data: { ...snapshot, role: "admin" } },
      { t: "snapshot", batch: 1, data: { ...snapshot, accounts: [{ ...snapshot.accounts[0], username: { a: 1 } }] } },
      { t: "snapshot", batch: 1, data: { ...snapshot, instances: [{ ...snapshot.instances[0], state: "weird" }] } },
      { t: "snapshot", batch: 1, data: { ...snapshot, bulkProgress: { ...snapshot.bulkProgress, items: [{ accountId: 1, userId: 1, stage: { kind: "failed" } }] } } },
      { t: "instancesUpdated", batch: 1, instances: "x" },
      { t: "metricsUpdated", batch: 1, metrics: [], systemUsage: null, windowsLocked: "no", vaultLocked: false, shutdownAt: null },
      { t: "rpcResult", reqId: "4", ok: true },
      { t: "accountsPage", batch: 1, offset: Number.NaN, accounts: [] },
      { t: "page", batch: 1, offset: 1, page: { list: "accounts", items: snapshot.accounts } },
      { t: "page", batch: 1, offset: 1, page: { list: "groups", items: snapshot.instances } },
      { t: "page", batch: 1, offset: 1, page: { list: "crashes" } },
      { t: "page", offset: 1, page: { list: "crashes", items: [] } },
    ];
    for (const ev of bad) expect(parseEvent(ev)).toBeNull();
  });
});

describe("PC names", () => {
  it("drop invisible, reordering and stacked characters", () => {
    expect(cleanName("  Gaming​‮ PC  ")).toBe("Gaming PC");
    expect(cleanName("á́́b")).toBe("áb");
    expect(cleanName("ㅤ⠀")).toBe("");
  });

  it("clash when they only look alike", () => {
    const key = nameKey("Gaming PC");
    for (const lookalike of ["gaming pc", "Gaming​PC", "Gаming PC", "Ｇaming PC", "Gaming PC"]) expect(nameKey(lookalike)).toBe(key);
    for (const lookalike of ["Ꮐaming PC", "Gaminց PC"]) expect(nameKey(lookalike)).toBe(key);
    expect(nameKey("Gaming PC 2")).not.toBe(key);
  });
});

describe("optional fields", () => {
  it("may be left out, so the views must treat a missing value like null", () => {
    const metrics = { instanceId: "i1", pid: 9, memoryMb: 1, privateMb: 1 };
    expect(parseEvent({ t: "metricsUpdated", batch: 1, metrics: [metrics], windowsLocked: false, vaultLocked: false })).not.toBeNull();
  });
});

describe("parseTargetParse", () => {
  it("accepts each target shape", () => {
    expect(parseTargetParse({ parsed: { kind: "target", target: { kind: "privateLink", placeId: 1, linkCode: "123" } } })).not.toBeNull();
    expect(parseTargetParse({ parsed: { kind: "invalid", reason: "no" } })).not.toBeNull();
  });

  it("rejects malformed results", () => {
    for (const data of [
      null,
      { parsed: null },
      { parsed: { kind: "target", target: { kind: "privateLink", placeId: "1", linkCode: "123" } } },
      { parsed: { kind: "target", target: { kind: "server", placeId: 1 } } },
      { parsed: { kind: "target", target: { kind: "teleport", placeId: 1 } } },
      { parsed: { kind: "invalid", reason: 5 } },
    ]) {
      expect(parseTargetParse(data)).toBeNull();
    }
  });
});

describe("reconnectDelay", () => {
  it("uses full jitter under a cap that grows to 60 s", () => {
    expect(reconnectDelay(0, () => 0)).toBe(500);
    expect(reconnectDelay(0, () => 1)).toBe(2_500);
    expect(reconnectDelay(3, () => 1)).toBe(16_500);
    expect(reconnectDelay(50, () => 1)).toBe(60_500);
    expect(reconnectDelay(50, () => 0.5)).toBe(30_500);
  });

  it("keeps backing off when the relay drops sockets soon after accepting them", () => {
    expect(backoffAfterClose(4, 1_000)).toBe(4);
    expect(backoffAfterClose(4, 29_999)).toBe(4);
    expect(backoffAfterClose(4, 30_000)).toBe(2);
    expect(backoffAfterClose(0, 60_000)).toBe(2);
  });
});
