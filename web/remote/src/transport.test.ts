import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  buildHandshake,
  concat,
  DIR_PC_TO_VIEWER,
  DIR_VIEWER_TO_PC,
  FRAME_HANDSHAKE,
  generateEphemeralEcdh,
  generateIdentityKeyPair,
  SessionCipher,
  toB64Url,
  uuidToBytes,
  verifyHandshake,
} from "./crypto";
import type { PairedPc } from "./storage";
import { RemoteManager } from "./transport";
import type { RemoteSnapshot } from "./types";

const PC_ID = "11111111-2222-4333-8444-555555555555";
const DEVICE_ID = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const enc = new TextEncoder();
const realSetTimeout = globalThis.setTimeout;
const realLocks = globalThis.navigator.locks;
const tick = () => new Promise((r) => realSetTimeout(r, 0));

const snapshot: RemoteSnapshot = {
  pcName: "Gaming PC",
  appVersion: "1.3.0",
  role: "control",
  vaultLocked: false,
  windowsLocked: false,
  accounts: [],
  groups: [{ id: 1, name: "Alts" }],
  favourites: [],
  instances: [],
  metrics: [],
  systemUsage: null,
  bulkProgress: null,
  shutdownAt: null,
  crashes: [],
};

class FakeWs {
  static OPEN = 1;
  static last: FakeWs;
  readyState = 0;
  binaryType = "";
  sent: (string | Uint8Array)[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: (() => void) | null = null;

  constructor(readonly url: string) {
    FakeWs.last = this;
  }
  send(data: string | Uint8Array) {
    this.sent.push(data);
  }
  close() {
    this.readyState = 3;
  }
  texts(): Record<string, unknown>[] {
    return this.sent.filter((d): d is string => typeof d === "string").map((d) => JSON.parse(d));
  }
  payloads(): Uint8Array[] {
    return this.sent.filter((d): d is Uint8Array => typeof d !== "string").map((d) => d.slice(17));
  }
  text(msg: Record<string, unknown>) {
    this.onmessage?.({ data: JSON.stringify(msg) });
  }
  binary(payload: Uint8Array) {
    this.onmessage?.({ data: concat(new Uint8Array([1]), uuidToBytes(PC_ID), payload).slice().buffer });
  }
}

async function until(cond: () => boolean) {
  for (let i = 0; i < 500 && !cond(); i++) await tick();
  expect(cond()).toBe(true);
}

async function settle() {
  for (let i = 0; i < 50; i++) await tick();
}

async function onlinePc() {
  const pcStatic = await generateIdentityKeyPair();
  const device = await generateIdentityKeyPair();
  const pc: PairedPc = {
    pcId: PC_ID,
    pcName: "Gaming PC",
    pcPubB64: toB64Url(pcStatic.publicKeyRaw),
    roomToken: "t".repeat(43),
    relayUrl: "wss://relay.test/ws/remote",
    role: "control",
    addedAt: 0,
    deviceId: DEVICE_ID,
    keyPair: device.keyPair,
  };
  const mgr = new RemoteManager();
  mgr.setPcs([pc]);
  const ws = FakeWs.last;
  ws.readyState = 1;
  ws.onopen?.();
  ws.text({ t: "pc_online", pcId: PC_ID });
  await until(() => ws.payloads().length === 1);
  const viewerEphem = await verifyHandshake(device.publicKeyRaw, ws.payloads()[0], DIR_VIEWER_TO_PC, PC_ID, DEVICE_ID);
  const pcEphem = await generateEphemeralEcdh();
  ws.binary(await buildHandshake(pcStatic.keyPair.privateKey, pcEphem.publicKeyRaw, DIR_PC_TO_VIEWER, PC_ID, DEVICE_ID));
  const cipher = await SessionCipher.derive(pcEphem.keyPair.privateKey, viewerEphem, pcEphem.publicKeyRaw, viewerEphem, true);
  const sendRaw = async (body: Uint8Array) => ws.binary(await cipher.seal(body));
  const send = (ev: unknown) => sendRaw(concat(new Uint8Array([1]), enc.encode(JSON.stringify(ev))));
  await until(() => ws.payloads().length === 2);
  await send({ t: "snapshot", batch: 1, data: snapshot });
  const rt = () => mgr.getRuntimes()[PC_ID];
  await until(() => rt().status === "online");
  return { mgr, ws, rt, send, sendRaw, pcStatic, device, cipher };
}

async function opsSince(ws: FakeWs, cipher: SessionCipher, from: number): Promise<unknown[]> {
  const ops: unknown[] = [];
  for (const p of ws.payloads().slice(from)) {
    if (p[0] === FRAME_HANDSHAKE) continue;
    ops.push(JSON.parse(new TextDecoder().decode((await cipher.open(p)).slice(1))).op);
  }
  return ops;
}

function previewFrame(instanceId: string): Uint8Array {
  const id = enc.encode(instanceId);
  return concat(new Uint8Array([2, id.length]), id, new Uint8Array(9));
}

function pictureFrame(instanceId: string, seq: number): Uint8Array {
  const id = enc.encode(instanceId);
  const meta = new Uint8Array(9);
  new DataView(meta.buffer).setUint32(0, seq, true);
  meta[8] = 1;
  return concat(new Uint8Array([2, id.length]), id, meta, new Uint8Array([0xff]));
}

describe("RemoteManager", () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    vi.stubGlobal("window", globalThis);
    vi.stubGlobal("document", { hidden: false, addEventListener() {}, removeEventListener() {} });
    vi.stubGlobal("requestAnimationFrame", (cb: () => void) => {
      cb();
      return 0;
    });
    vi.stubGlobal("cancelAnimationFrame", () => {});
    vi.stubGlobal("WebSocket", FakeWs);
    vi.stubGlobal("navigator", {});
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("keeps an established session when the relay announces the PC again", async () => {
    const { ws, rt } = await onlinePc();
    const sent = ws.sent.length;
    ws.text({ t: "pc_online", pcId: PC_ID });
    await settle();
    expect(rt().status).toBe("online");
    expect(ws.sent.length).toBe(sent);
  });

  it("asks the relay to restart the session, keeping its slot, when the PC's handshake never arrives", async () => {
    const device = await generateIdentityKeyPair();
    const mgr = new RemoteManager();
    mgr.setPcs([{ pcId: PC_ID, pcName: "PC", pcPubB64: "AA", roomToken: "t".repeat(43), relayUrl: "wss://r", role: "control", addedAt: 0, deviceId: DEVICE_ID, keyPair: device.keyPair }]);
    const ws = FakeWs.last;
    ws.readyState = 1;
    ws.onopen?.();
    ws.text({ t: "pc_online", pcId: PC_ID });
    await until(() => ws.payloads().length === 1);
    vi.advanceTimersByTime(10_000);
    expect(ws.texts().filter((m) => m.t === "unsubscribe")).toHaveLength(0);
    vi.advanceTimersByTime(1_000);
    const subs = ws.texts().filter((m) => m.t === "subscribe");
    expect(subs).toHaveLength(2);
    expect(subs[1].pcs).toEqual([{ pcId: PC_ID, deviceId: DEVICE_ID, roomToken: "t".repeat(43), restart: true }]);
    ws.text({ t: "pc_online", pcId: PC_ID });
    await until(() => ws.payloads().length === 2);
    expect(ws.payloads()[1][0]).toBe(FRAME_HANDSHAKE);
    mgr.destroy();
  });

  it("asks for the snapshot again when only the snapshot was lost", async () => {
    const pcStatic = await generateIdentityKeyPair();
    const device = await generateIdentityKeyPair();
    const mgr = new RemoteManager();
    mgr.setPcs([{ pcId: PC_ID, pcName: "PC", pcPubB64: toB64Url(pcStatic.publicKeyRaw), roomToken: "t".repeat(43), relayUrl: "wss://r", role: "control", addedAt: 0, deviceId: DEVICE_ID, keyPair: device.keyPair }]);
    const ws = FakeWs.last;
    ws.readyState = 1;
    ws.onopen?.();
    ws.text({ t: "pc_online", pcId: PC_ID });
    await until(() => ws.payloads().length === 1);
    const viewerEphem = await verifyHandshake(device.publicKeyRaw, ws.payloads()[0], DIR_VIEWER_TO_PC, PC_ID, DEVICE_ID);
    const pcEphem = await generateEphemeralEcdh();
    ws.binary(await buildHandshake(pcStatic.keyPair.privateKey, pcEphem.publicKeyRaw, DIR_PC_TO_VIEWER, PC_ID, DEVICE_ID));
    const cipher = await SessionCipher.derive(pcEphem.keyPair.privateKey, viewerEphem, pcEphem.publicKeyRaw, viewerEphem, true);
    await until(() => ws.payloads().length === 2);
    vi.advanceTimersByTime(10_000);
    await until(() => ws.payloads().length >= 6);
    const handshakes = ws.payloads().filter((p) => p[0] === FRAME_HANDSHAKE);
    expect(handshakes).toEqual([ws.payloads()[0], ws.payloads()[0]]);
    const ops: unknown[] = [];
    for (const p of ws.payloads().filter((p) => p[0] !== FRAME_HANDSHAKE)) ops.push(JSON.parse(new TextDecoder().decode((await cipher.open(p)).slice(1))).op);
    expect(ops).toContain("requestSnapshot");
    expect(ws.texts().filter((m) => m.t === "unsubscribe")).toHaveLength(0);
    mgr.destroy();
  });

  it("answers a fresh handshake from a PC that reconnected with a new key", async () => {
    const { mgr, ws, rt, pcStatic, device } = await onlinePc();
    const before = ws.payloads().length;
    const pcEphem = await generateEphemeralEcdh();
    ws.binary(await buildHandshake(pcStatic.keyPair.privateKey, pcEphem.publicKeyRaw, DIR_PC_TO_VIEWER, PC_ID, DEVICE_ID));
    await until(() => ws.payloads().length === before + 2);
    expect(rt().status).toBe("connecting");
    const viewerEphem = await verifyHandshake(device.publicKeyRaw, ws.payloads()[before], DIR_VIEWER_TO_PC, PC_ID, DEVICE_ID);
    const cipher = await SessionCipher.derive(pcEphem.keyPair.privateKey, viewerEphem, pcEphem.publicKeyRaw, viewerEphem, true);
    expect((await cipher.open(ws.payloads()[before + 1]))[0]).toBe(1);
    ws.binary(await cipher.seal(concat(new Uint8Array([1]), enc.encode(JSON.stringify({ t: "snapshot", batch: 1, data: snapshot })))));
    await until(() => rt().status === "online");
    mgr.destroy();
  });

  it("ignores event types it doesn't know", async () => {
    const { mgr, rt, send } = await onlinePc();
    for (let i = 0; i < 10; i++) await send({ t: "fromANewerPc" });
    await settle();
    expect(rt().status).toBe("online");
    expect(rt().problem).toBeNull();
    mgr.destroy();
  });

  it("ends the session after repeated invalid events and stops subscribing to that PC", async () => {
    const { mgr, ws, rt, send } = await onlinePc();
    for (let i = 0; i < 4; i++) await send({ t: "snapshot", batch: 1, data: 5 });
    await settle();
    expect(rt().status).toBe("online");
    await send({ t: "snapshot", batch: 1, data: 5 });
    await until(() => rt().status === "offline");
    expect(rt().problem).toMatch(/can't read/);
    expect(ws.texts().filter((m) => m.t === "unsubscribe")).toHaveLength(1);
    mgr.setPcs([rt().pc]);
    expect(ws.texts().at(-1)).toMatchObject({ t: "subscribe", pcs: [] });
    mgr.destroy();
  });

  it("strikes previews nobody asked for, but not ones still in flight after a change", async () => {
    const { mgr, ws, rt, send, sendRaw, cipher } = await onlinePc();
    mgr.setPreviewTargets(PC_ID, [{ instanceId: "a", maxW: 320, fps: 15 }]);
    await settle();
    const read = ws.payloads().length;
    mgr.setPreviewTargets(PC_ID, []);
    await until(() => ws.payloads().length > read);
    const req = JSON.parse(new TextDecoder().decode((await cipher.open(ws.payloads()[read])).slice(1)));
    await send({ t: "rpcResult", reqId: req.reqId, ok: true, data: { subscribed: 0 } });
    await settle();
    for (let i = 0; i < 60; i++) await sendRaw(previewFrame("a"));
    await settle();
    expect(rt().status).toBe("online");
    for (let i = 0; i < 51; i++) await sendRaw(previewFrame("ghost"));
    await until(() => rt().status === "offline");
    expect(rt().problem).toMatch(/did not ask for/);
    mgr.destroy();
  });

  it("acks pictures at once until the PC's ack budget runs out, then only the newest", async () => {
    const { mgr, ws, sendRaw, cipher } = await onlinePc();
    mgr.setPreviewTargets(PC_ID, [{ instanceId: "a", maxW: 320, fps: 30 }]);
    let read = ws.payloads().length;
    const acks: number[] = [];
    const ackedUpTo = async (n: number) => {
      for (let i = 0; i < 500 && acks.length < n; i++) {
        for (const p of ws.payloads().slice(read)) {
          read++;
          const msg = JSON.parse(new TextDecoder().decode((await cipher.open(p)).slice(1)));
          if (msg.op === "previewAck") acks.push(msg.seq);
        }
        if (acks.length < n) await tick();
      }
    };
    for (let seq = 1; seq <= 100; seq++) {
      await sendRaw(pictureFrame("a", seq));
      if (seq <= 90) await ackedUpTo(seq);
    }
    await settle();
    await ackedUpTo(90);
    expect(acks).toHaveLength(90);
    vi.advanceTimersByTime(20);
    await ackedUpTo(91);
    expect(acks).toEqual([...Array.from({ length: 90 }, (_, i) => i + 1), 100]);
    mgr.destroy();
  });

  it("appends list pages only where they continue the list, and reloads a list that lost a page", async () => {
    const { mgr, ws, rt, send, cipher } = await onlinePc();
    const read = ws.payloads().length;
    await send({ t: "page", batch: 1, offset: 1, page: { list: "groups", items: [{ id: 2, name: "Main" }] } });
    await send({ t: "page", batch: 1, offset: 5, page: { list: "groups", items: [{ id: 3, name: "Stale" }] } });
    await until(() => rt().snapshot?.groups.length === 2);
    await settle();
    expect(rt().snapshot?.groups.map((g) => g.id)).toEqual([1, 2]);
    expect(rt().lastNotice?.message).toMatch(/got lost/);
    const ops: unknown[] = [];
    for (const p of ws.payloads().slice(read)) ops.push(JSON.parse(new TextDecoder().decode((await cipher.open(p)).slice(1))).op);
    expect(ops).toContain("requestSnapshot");
    mgr.destroy();
  });

  it("does not strike previews of targets the PC refused to change", async () => {
    const { mgr, ws, rt, send, sendRaw, cipher } = await onlinePc();
    mgr.setPreviewTargets(PC_ID, [{ instanceId: "a", maxW: 320, fps: 15 }]);
    await settle();
    const read = ws.payloads().length;
    mgr.setPreviewTargets(PC_ID, []);
    await until(() => ws.payloads().length > read);
    const req = JSON.parse(new TextDecoder().decode((await cipher.open(ws.payloads()[read])).slice(1)));
    expect(req.op).toBe("subscribePreviews");
    await send({ t: "rpcResult", reqId: req.reqId, ok: false, error: "slow down", code: "rate_limited" });
    vi.advanceTimersByTime(5_000);
    for (let i = 0; i < 120; i++) await sendRaw(previewFrame("a"));
    await settle();
    expect(rt().status).toBe("online");
    mgr.destroy();
  });

  it("strikes the old targets' previews once every retry of a change was refused", async () => {
    const { mgr, ws, rt, send, sendRaw, cipher } = await onlinePc();
    const read = { i: 0 };
    const nextRequest = async (op: string) => {
      for (let tries = 0; tries < 500; tries++) {
        while (read.i < ws.payloads().length) {
          const p = ws.payloads()[read.i++];
          if (p[0] === FRAME_HANDSHAKE) continue;
          const msg = JSON.parse(new TextDecoder().decode((await cipher.open(p)).slice(1)));
          if (msg.op === op) return msg;
        }
        await tick();
      }
      throw new Error(`no ${op} request`);
    };
    mgr.setPreviewTargets(PC_ID, [{ instanceId: "a", maxW: 320, fps: 15 }]);
    await send({ t: "rpcResult", reqId: (await nextRequest("subscribePreviews")).reqId, ok: true, data: { subscribed: 1 } });
    mgr.setPreviewTargets(PC_ID, []);
    for (let attempt = 0; attempt < 5; attempt++) {
      await send({ t: "rpcResult", reqId: (await nextRequest("subscribePreviews")).reqId, ok: false, error: "slow down", code: "rate_limited" });
      await settle();
      if (attempt < 4) vi.advanceTimersByTime(1_000 * 2 ** attempt);
    }
    for (let i = 0; i < 150; i++) {
      await sendRaw(previewFrame("a"));
      vi.advanceTimersByTime(100);
    }
    await until(() => rt().status === "offline");
    expect(rt().problem).toMatch(/did not ask for/);
    mgr.destroy();
  });

  it("reloads a list instead of mixing in pages of an update whose first page was lost", async () => {
    const { mgr, ws, rt, send, cipher } = await onlinePc();
    const read = ws.payloads().length;
    await send({ t: "page", batch: 1, offset: 0, page: { list: "groups", items: [{ id: 9, name: "New" }] } });
    await until(() => rt().lastNotice !== null);
    await settle();
    expect(rt().snapshot?.groups.map((g) => g.id)).toEqual([1]);
    expect(await opsSince(ws, cipher, read)).toContain("requestSnapshot");
    mgr.destroy();
  });

  it("reloads a list instead of adding a page of another update that happens to continue it", async () => {
    const { mgr, ws, rt, send, cipher } = await onlinePc();
    const read = ws.payloads().length;
    await send({ t: "page", batch: 2, offset: 1, page: { list: "groups", items: [{ id: 11, name: "Second of a lost update" }] } });
    await until(() => rt().lastNotice !== null);
    await settle();
    expect(rt().snapshot?.groups.map((g) => g.id)).toEqual([1]);
    expect(await opsSince(ws, cipher, read)).toContain("requestSnapshot");
    await send({ t: "accountsUpdated", batch: 3, accounts: [], groups: [{ id: 5, name: "New" }] });
    await send({ t: "page", batch: 3, offset: 1, page: { list: "groups", items: [{ id: 6, name: "Next" }] } });
    await until(() => rt().snapshot?.groups.length === 2);
    expect(rt().snapshot?.groups.map((g) => g.id)).toEqual([5, 6]);
    mgr.destroy();
  });

  it("strikes the old targets' previews once a PC that never confirms has had every retry, however often it is asked again", async () => {
    const { mgr, rt, send, sendRaw } = await onlinePc();
    mgr.setPreviewTargets(PC_ID, [{ instanceId: "a", maxW: 320, fps: 15 }]);
    for (let round = 0; round < 20; round++) {
      mgr.setPreviewTargets(PC_ID, round % 2 ? [] : [{ instanceId: "b", maxW: 320, fps: 15 }]);
      await send({ t: "rpcResult", reqId: 1, ok: true });
      vi.advanceTimersByTime(3_000);
      await tick();
    }
    await settle();
    expect(rt().status).toBe("online");
    for (let i = 0; i < 60; i++) await sendRaw(previewFrame("a"));
    await until(() => rt().status === "offline");
    expect(rt().problem).toMatch(/did not ask for/);
    mgr.destroy();
  });

  it("waits for a PC that turned it away while paused to come back, instead of retrying", async () => {
    const { mgr, ws, rt } = await onlinePc();
    ws.text({ t: "pc_err", pcId: PC_ID, code: "paused" });
    await settle();
    expect(rt().status).toBe("paused");
    expect(rt().problem).toBeNull();
    const subscribes = () => ws.texts().filter((m) => m.t === "subscribe").length;
    const before = subscribes();
    ws.text({ t: "pc_offline", pcId: PC_ID });
    vi.advanceTimersByTime(300_000);
    await settle();
    expect(subscribes()).toBe(before);
    expect(rt().status).toBe("paused");
    ws.text({ t: "pc_online", pcId: PC_ID });
    await until(() => rt().status === "connecting");
    expect(rt().problem).toBeNull();
    mgr.destroy();
  });

  it("says why a PC with too many sessions of this device turned it away, and tries again", async () => {
    const { mgr, ws, rt } = await onlinePc();
    ws.text({ t: "pc_err", pcId: PC_ID, code: "device_sessions" });
    await settle();
    expect(rt().status).toBe("deviceBusy");
    expect(rt().problem).toBeNull();
    vi.advanceTimersByTime(20_000);
    await settle();
    const last = ws.texts().filter((m) => m.t === "subscribe").at(-1) as { pcs: { restart?: boolean }[] };
    expect(last.pcs[0].restart).toBe(true);
    ws.text({ t: "pc_online", pcId: PC_ID });
    await until(() => ws.payloads().filter((p) => p[0] === FRAME_HANDSHAKE).length === 2);
    expect(rt().status).toBe("deviceBusy");
    mgr.destroy();
  });

  it("still says the device is busy when the relay closes the socket of a turned-away session", async () => {
    const { mgr, ws, rt } = await onlinePc();
    ws.text({ t: "pc_err", pcId: PC_ID, code: "device_sessions" });
    await settle();
    ws.text({ t: "err", code: "no_purpose" });
    ws.readyState = 3;
    ws.onclose?.();
    expect(rt().status).toBe("deviceBusy");
    vi.advanceTimersByTime(70_000);
    const next = FakeWs.last;
    expect(next).not.toBe(ws);
    next.readyState = 1;
    next.onopen?.();
    next.text({ t: "pc_online", pcId: PC_ID });
    await until(() => next.payloads().length === 1);
    expect(rt().status).toBe("deviceBusy");
    mgr.destroy();
  });

  it("takes a PC back once after another socket of this device replaced it", async () => {
    const { mgr, ws, rt } = await onlinePc();
    ws.text({ t: "pc_err", pcId: PC_ID, code: "replaced" });
    await settle();
    expect(rt().status).toBe("connecting");
    expect(ws.texts().filter((m) => m.t === "unsubscribe")).toHaveLength(0);
    vi.advanceTimersByTime(10_000);
    await settle();
    vi.advanceTimersByTime(1_000);
    await settle();
    const last = ws.texts().filter((m) => m.t === "subscribe").at(-1) as { pcs: { restart?: boolean }[] };
    expect(last.pcs[0].restart).toBe(true);
    mgr.destroy();
  });

  it("ignores a replacement reported on a socket it already left", async () => {
    const { mgr, ws, rt } = await onlinePc();
    ws.readyState = 3;
    ws.onclose?.();
    ws.text({ t: "pc_err", pcId: PC_ID, code: "replaced" });
    vi.advanceTimersByTime(70_000);
    const next = FakeWs.last;
    next.readyState = 1;
    next.onopen?.();
    next.text({ t: "pc_err", pcId: PC_ID, code: "replaced" });
    await settle();
    expect(rt().status).toBe("connecting");
    mgr.destroy();
  });

  it("frees its tab slot for another tab when it lets go of a replaced PC", async () => {
    vi.stubGlobal("navigator", { locks: realLocks });
    const { mgr, ws } = await onlinePc();
    const others = Array.from({ length: 3 }, () => {
      const other = new RemoteManager();
      other.setPcs([{ ...mgr.getRuntimes()[PC_ID].pc }]);
      return other;
    });
    await settle();
    const waiting = others[2].getRuntimes()[PC_ID];
    expect(waiting.status).toBe("tooManyTabs");
    ws.text({ t: "pc_err", pcId: PC_ID, code: "replaced" });
    ws.text({ t: "pc_err", pcId: PC_ID, code: "replaced" });
    await until(() => others[2].getRuntimes()[PC_ID].status === "connecting");
    for (const m of [mgr, ...others]) m.destroy();
  });

  it("lets go of a PC that keeps being replaced by other tabs, and says so", async () => {
    const { mgr, ws, rt } = await onlinePc();
    ws.text({ t: "pc_err", pcId: PC_ID, code: "replaced" });
    vi.advanceTimersByTime(60_000);
    ws.text({ t: "pc_err", pcId: PC_ID, code: "replaced" });
    await settle();
    expect(rt().status).toBe("offline");
    expect(rt().problem).toMatch(/too many tabs/);
    expect(ws.texts().at(-1)).toEqual({ t: "unsubscribe", pcId: PC_ID });
    const subscribes = ws.texts().filter((m) => m.t === "subscribe").length;
    vi.advanceTimersByTime(300_000);
    await settle();
    expect(ws.texts().filter((m) => m.t === "subscribe")).toHaveLength(subscribes);
    mgr.destroy();
  });

  it("asks for the snapshot again once the backoff ends when a list loses a page during it", async () => {
    const { mgr, ws, rt, send, cipher } = await onlinePc();
    const gap = () => send({ t: "page", batch: 1, offset: 5, page: { list: "groups", items: [{ id: 3, name: "Lost" }] } });
    const seen = { read: ws.payloads().length, requests: 0 };
    const requests = async () => {
      const from = seen.read;
      seen.read = ws.payloads().length;
      const ops = await opsSince(ws, cipher, from);
      seen.requests += ops.filter((op) => op === "requestSnapshot").length;
      return seen.requests;
    };
    await gap();
    await until(() => rt().lastNotice !== null);
    await settle();
    expect(await requests()).toBe(1);
    await send({ t: "snapshot", batch: 1, data: snapshot });
    await until(() => rt().lastNotice === null);
    await gap();
    await until(() => rt().lastNotice !== null);
    await settle();
    expect(await requests()).toBe(1);
    vi.advanceTimersByTime(2_000);
    await settle();
    expect(await requests()).toBe(2);
    mgr.destroy();
  });

  it("renders at most once a frame however many events arrive", async () => {
    const { mgr, send } = await onlinePc();
    const frames: (() => void)[] = [];
    vi.stubGlobal("requestAnimationFrame", (cb: () => void) => frames.push(cb));
    let renders = 0;
    mgr.subscribe(() => renders++);
    for (let i = 0; i < 5; i++) await send({ t: "notice", error: true, message: `n${i}` });
    await settle();
    expect(renders).toBe(0);
    expect(frames).toHaveLength(1);
    frames.shift()?.();
    expect(renders).toBe(1);
    mgr.destroy();
  });

  it("starts over when an online PC goes silent without a pc_offline", async () => {
    const { mgr, ws, rt } = await onlinePc();
    for (let s = 0; s < 3; s++) {
      vi.advanceTimersByTime(5_000);
      await settle();
    }
    expect(rt().status).toBe("connecting");
    vi.advanceTimersByTime(1_000);
    const last = ws.texts().filter((m) => m.t === "subscribe").at(-1) as { pcs: { restart?: boolean }[] };
    expect(last.pcs[0].restart).toBe(true);
    mgr.destroy();
  });

  it("stops at 10,000 items with a notice instead of reloading the list forever", async () => {
    const { mgr, ws, rt, send, cipher } = await onlinePc();
    const read = ws.payloads().length;
    const items = (from: number, n: number) => Array.from({ length: n }, (_, i) => ({ id: from + i, name: "g" }));
    await send({ t: "page", batch: 1, offset: 1, page: { list: "groups", items: items(2, 9_999) } });
    await send({ t: "page", batch: 1, offset: 10_000, page: { list: "groups", items: items(10_001, 5) } });
    await until(() => rt().lastNotice !== null);
    await settle();
    expect(rt().snapshot?.groups).toHaveLength(10_000);
    expect(rt().lastNotice).toMatchObject({ error: true, message: expect.stringMatching(/10,000/) });
    const ops: unknown[] = [];
    for (const p of ws.payloads().slice(read)) ops.push(JSON.parse(new TextDecoder().decode((await cipher.open(p)).slice(1))).op);
    expect(ops).not.toContain("requestSnapshot");
    mgr.destroy();
  });

  it("shows a full PC as waiting for a slot", async () => {
    const device = await generateIdentityKeyPair();
    const mgr = new RemoteManager();
    mgr.setPcs([{ pcId: PC_ID, pcName: "PC", pcPubB64: "AA", roomToken: "t".repeat(43), relayUrl: "wss://r", role: "control", addedAt: 0, deviceId: DEVICE_ID, keyPair: device.keyPair }]);
    const ws = FakeWs.last;
    ws.readyState = 1;
    ws.onopen?.();
    ws.text({ t: "pc_err", pcId: PC_ID, code: "full" });
    await until(() => mgr.getRuntimes()[PC_ID].problem !== null);
    expect(mgr.getRuntimes()[PC_ID].problem).toMatch(/16 dashboards/);
    mgr.destroy();
  });

  it("follows one PC from at most 3 tabs of this browser, and lets a 4th in when one closes", async () => {
    vi.stubGlobal("navigator", { locks: realLocks });
    const device = await generateIdentityKeyPair();
    const pc: PairedPc = { pcId: PC_ID, pcName: "PC", pcPubB64: "AA", roomToken: "t".repeat(43), relayUrl: "wss://r", role: "control", addedAt: 0, deviceId: DEVICE_ID, keyPair: device.keyPair };
    const tabs = Array.from({ length: 4 }, () => {
      const mgr = new RemoteManager();
      mgr.setPcs([pc]);
      return { mgr, ws: FakeWs.last };
    });
    await settle();
    const subscribed = (ws: FakeWs) => (ws.texts().filter((m) => m.t === "subscribe").at(-1) as { pcs: unknown[] } | undefined)?.pcs.length ?? 0;
    for (const { ws } of tabs) {
      ws.readyState = 1;
      ws.onopen?.();
    }
    expect(tabs.map(({ ws }) => subscribed(ws))).toEqual([1, 1, 1, 0]);
    const fourth = tabs[3];
    expect(fourth.mgr.getRuntimes()[PC_ID].status).toBe("tooManyTabs");
    fourth.ws.onclose?.();
    expect(fourth.mgr.getRuntimes()[PC_ID].status).toBe("tooManyTabs");
    const reopened = FakeWs.last;
    tabs[0].mgr.destroy();
    await settle();
    expect(fourth.mgr.getRuntimes()[PC_ID].status).toBe("connecting");
    vi.advanceTimersByTime(70_000);
    const ws = FakeWs.last;
    expect(ws).not.toBe(reopened);
    ws.readyState = 1;
    ws.onopen?.();
    expect(subscribed(ws)).toBe(1);
    for (const { mgr } of tabs) mgr.destroy();
  });

  it("starts over on a new socket when the old one closed during the handshake setup", async () => {
    const device = await generateIdentityKeyPair();
    const mgr = new RemoteManager();
    mgr.setPcs([{ pcId: PC_ID, pcName: "PC", pcPubB64: "AA", roomToken: "t".repeat(43), relayUrl: "wss://r", role: "control", addedAt: 0, deviceId: DEVICE_ID, keyPair: device.keyPair }]);
    const first = FakeWs.last;
    first.readyState = 1;
    first.onopen?.();
    first.text({ t: "pc_online", pcId: PC_ID });
    first.readyState = 3;
    first.onclose?.();
    vi.advanceTimersByTime(70_000);
    const second = FakeWs.last;
    expect(second).not.toBe(first);
    second.readyState = 1;
    second.onopen?.();
    second.text({ t: "pc_online", pcId: PC_ID });
    await until(() => second.payloads().length === 1);
    expect(second.payloads()[0][0]).toBe(FRAME_HANDSHAKE);
    mgr.destroy();
  });
});
