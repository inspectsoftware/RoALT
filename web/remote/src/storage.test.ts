import { afterEach, describe, expect, it, vi } from "vitest";
import { generateIdentityKeyPair } from "./crypto";
import { devicesReplacedBy, fallBackStale, UNCONFIRMED_FALLBACK_MS, type PairedPc } from "./storage";

const keyPair = {} as CryptoKeyPair;
const pc = (over: Partial<PairedPc>): PairedPc => ({
  pcId: "p",
  pcName: "PC",
  pcPubB64: "AA",
  roomToken: "new",
  relayUrl: "wss://r",
  role: "control",
  addedAt: 0,
  deviceId: "d2",
  keyPair,
  ...over,
});

function fakeIndexedDb() {
  const data = new Map<string, unknown>();
  let queue = Promise.resolve();
  const db = {
    objectStoreNames: { contains: () => true },
    transaction() {
      const ops: (() => void)[] = [];
      const writes = new Map<string, unknown>();
      let aborted = false;
      const request = (run: (req: { result?: unknown; onsuccess?: () => void }) => void) => {
        const req: { result?: unknown; onsuccess?: () => void } = {};
        ops.push(() => run(req));
        return req;
      };
      const tx = {
        error: null,
        oncomplete: null as null | (() => void),
        onerror: null as null | (() => void),
        onabort: null as null | (() => void),
        abort: () => (aborted = true),
        objectStore: () => ({
          get: (key: string) =>
            request((req) => {
              req.result = writes.has(key) ? writes.get(key) : data.get(key);
              req.onsuccess?.();
            }),
          put: (value: unknown, key: string) =>
            request((req) => {
              writes.set(key, value);
              req.onsuccess?.();
            }),
        }),
      };
      queue = queue.then(async () => {
        await Promise.resolve();
        while (ops.length && !aborted) {
          ops.shift()?.();
          await Promise.resolve();
        }
        if (aborted) return tx.onabort?.();
        for (const [k, v] of writes) data.set(k, v);
        tx.oncomplete?.();
      });
      return tx;
    },
  };
  return {
    open: () => {
      const req: { result?: unknown; onsuccess?: () => void } = { result: db };
      void Promise.resolve().then(() => req.onsuccess?.());
      return req;
    },
  };
}

describe("re-pairing", () => {
  it("swaps to the pairing it replaced when the new one never had a session, keeping both", () => {
    const previous = { deviceId: "d1", keyPair, roomToken: "old", role: "readonly" as const };
    expect(fallBackStale([pc({ previous })], UNCONFIRMED_FALLBACK_MS - 1)).toBeNull();
    const [back] = fallBackStale([pc({ previous })], UNCONFIRMED_FALLBACK_MS) ?? [];
    expect(back).toMatchObject({ deviceId: "d1", roomToken: "old", role: "readonly", addedAt: UNCONFIRMED_FALLBACK_MS });
    expect(back.previous).toMatchObject({ deviceId: "d2", roomToken: "new", role: "control" });
    const [again] = fallBackStale([back], UNCONFIRMED_FALLBACK_MS * 2) ?? [];
    expect(again).toMatchObject({ deviceId: "d2", roomToken: "new", previous: { deviceId: "d1" } });
    expect(fallBackStale([pc({})], UNCONFIRMED_FALLBACK_MS * 10)).toBeNull();
  });

  it("proves both devices it holds for a PC, so the PC drops whichever of them it kept", () => {
    const previous = { deviceId: "d1", keyPair, roomToken: "old", role: "readonly" as const };
    const ids = (list: PairedPc[], deviceId: string, pcPubB64 = "AA") => devicesReplacedBy(list, { pcId: "p", pcPubB64, deviceId }).map((d) => d.deviceId);
    expect(ids([pc({ previous })], "d3")).toEqual(["d2", "d1"]);
    expect(ids([pc({})], "d3")).toEqual(["d2"]);
    expect(ids([pc({ previous })], "d1")).toEqual(["d2"]);
    expect(ids([pc({ previous })], "d2")).toEqual([]);
    expect(ids([pc({ previous })], "d3", "BB")).toEqual([]);
  });
});

describe("paired PC storage", () => {
  afterEach(() => vi.unstubAllGlobals());

  const freshStorage = async (indexedDb: unknown) => {
    vi.stubGlobal("indexedDB", indexedDb);
    vi.resetModules();
    return import("./storage");
  };

  const repaired = async () => {
    const storage = await freshStorage(fakeIndexedDb());
    const old = await generateIdentityKeyPair();
    const cur = await generateIdentityKeyPair();
    await storage.upsertPairedPc(pc({ deviceId: "d1", roomToken: "old", keyPair: old.keyPair }));
    await storage.upsertPairedPc(pc({ keyPair: cur.keyPair }));
    expect((await storage.loadPairedPcs())[0].previous?.deviceId).toBe("d1");
    return storage;
  };

  it("does not bring a forgotten PC back when a confirm races the Forget", async () => {
    const storage = await repaired();
    await Promise.all([storage.removePairedPc("p"), storage.confirmPairedPc("p", "d2"), storage.fallBackStalePairings(UNCONFIRMED_FALLBACK_MS)]);
    expect(await storage.loadPairedPcs()).toEqual([]);
  });

  it("confirms whichever of the two devices had a session", async () => {
    const storage = await repaired();
    const [kept] = await storage.confirmPairedPc("p", "d1");
    expect(kept).toMatchObject({ deviceId: "d1", roomToken: "old", previous: undefined });
    const [again] = await storage.confirmPairedPc("p", "d2");
    expect(again).toMatchObject({ deviceId: "d1", previous: undefined });
  });

  it("refuses another re-pairing until the last one had a session, so no device the PC keeps is dropped", async () => {
    const storage = await repaired();
    const next = await generateIdentityKeyPair();
    await expect(storage.upsertPairedPc(pc({ deviceId: "d3", keyPair: next.keyPair }))).rejects.toThrow(/hasn't connected yet/);
    const [stored] = await storage.loadPairedPcs();
    expect([stored.deviceId, stored.previous?.deviceId]).toEqual(["d2", "d1"]);
    await storage.confirmPairedPc("p", "d2");
    const [repairedAgain] = await storage.upsertPairedPc(pc({ deviceId: "d3", keyPair: next.keyPair }));
    expect([repairedAgain.deviceId, repairedAgain.previous?.deviceId]).toEqual(["d3", "d2"]);
  });

  it("fails instead of hanging when another tab blocks opening the database, and can try again", async () => {
    const storage = await freshStorage({
      open: () => {
        const req: { onblocked?: () => void } = {};
        void Promise.resolve().then(() => req.onblocked?.());
        return req;
      },
    });
    await expect(storage.loadPairedPcs()).rejects.toThrow(/other tab/);
    vi.stubGlobal("indexedDB", fakeIndexedDb());
    expect(await storage.loadPairedPcs()).toEqual([]);
  });
});
