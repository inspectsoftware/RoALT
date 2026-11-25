/**
 * Multiplexed multi-PC E2EE WebSocket transport and QR/Manual pairing flows.
 */
import {
  buildHandshake,
  bytesToUuid,
  concat,
  deriveManualPairKeys,
  deriveQrPairKey,
  DIR_PC_TO_VIEWER,
  DIR_VIEWER_TO_PC,
  FRAME_DATA,
  FRAME_HANDSHAKE,
  fromB64Url,
  generateEphemeralEcdh,
  openPairBlob,
  pairCommit,
  PAIR_MODE_MANUAL,
  PAIR_MODE_QR,
  pairTranscript,
  sealPairBlob,
  SessionCipher,
  signDeviceBinding,
  toB64Url,
  uuidToBytes,
  verifyHandshake,
  verifyPcBinding,
} from "./crypto";
import { devicesReplacedBy, loadPairedPcs, newDeviceIdentity, whyNotStored, type DeviceIdentity, type PairedPc } from "./storage";
import type { PcConnStatus, PreviewFrame, RemoteEvent, RemotePage, RemotePreviewTarget, RemoteSnapshot } from "./types";
import { cleanName, parseEvent } from "./validate";

const PROTO = 2;
const BIN_VERSION = 0x01;
const BIN_HEADER_LEN = 17;
const APP_KIND_JSON = 0x01;
const APP_KIND_PREVIEW = 0x02;
const LIVENESS_PING_MS = 5_000;
const SESSION_SILENCE_MS = 12_000;
const MAX_QUEUED_FRAMES = 64;
const MAX_LIST_ITEMS = 10_000;
const PREVIEW_ACKS_PER_SEC = 90;
const HANDSHAKE_WAIT_MS = 10_000;
const RESUBSCRIBE_DELAY_MS = 1_000;
const RELAY_PING_MS = 25_000;
const STRIKE_WINDOW_MS = 10_000;
const MAX_STRIKES = 50;
const MAX_BAD_EVENTS = 5;
const BAD_EVENT_WINDOW_MS = 60_000;
const PREVIEW_GRACE_MS = 2_000;
const PREVIEW_RETRIES = 5;
const PREVIEW_RPC_MS = 8_000;
const PREVIEW_ASK_MS = PREVIEW_GRACE_MS + PREVIEW_RETRIES * PREVIEW_RPC_MS + (2 ** (PREVIEW_RETRIES - 1) - 1) * 1_000;
const STABLE_MS = 30_000;
const AFTER_STABLE_ATTEMPT = 2;
const HIDDEN_CLOSE_MS = 30_000;
const RESYNC_MIN_MS = 2_000;
const RESYNC_MAX_MS = 60_000;
const TABS_PER_PC = 3;
const REPLACED_AGAIN_MS = 10 * 60_000;
const KNOWN_EVENTS = new Set(["snapshot", "accountsUpdated", "accountsPage", "page", "instancesUpdated", "metricsUpdated", "bulkProgress", "rpcResult", "notice", "webRtcSignal"]);
const PC_FULL = "This PC already has 16 dashboards open. This one connects as soon as one of them closes.";
const LIST_GAP = "Part of a list from this PC got lost on the way. Loading it again.";
const LIST_TOO_LONG = "This PC has more than 10,000 items in a list. Only the first 10,000 are shown.";
const BAD_DATA = "This PC sent data this dashboard can't read. Update RoALT on the PC and reload this page.";
const BAD_HANDSHAKE = "This PC's encryption check failed: it may run an older RoALT, or its key changed. Update RoALT on the PC, or pair it again.";
const MISBEHAVING = "This PC kept sending data this dashboard did not ask for, so it was disconnected. Reload the page to try again.";
const REPLACED = "This PC is open in too many tabs of this browser, so this tab let go of it. Close one of the others and reload this page.";

const enc = new TextEncoder();
const dec = new TextDecoder();

export type PcRuntime = {
  pc: PairedPc;
  status: PcConnStatus;
  snapshot: RemoteSnapshot | null;
  previews: Record<string, PreviewFrame>;
  lastNotice: { error: boolean; message: string; at: number } | null;
  problem: string | null;
  previewsOff: boolean;
};

export type RelayIssue = { code: string; at: number };

export class RpcError extends Error {
  constructor(
    message: string,
    readonly code: string | null,
  ) {
    super(message);
  }
}

type SessionState = {
  pc: PairedPc;
  ephemPriv: CryptoKey;
  ephemPubRaw: Uint8Array;
  handshake: Uint8Array;
  cipher: SessionCipher | null;
  badEvents: { since: number; count: number };
  lastRx: number;
};

type PreviewWatcher = {
  draw: (jpeg: Blob) => Promise<void>;
  busy: boolean;
  next: { jpeg: Blob; seq: number } | null;
};

type PendingRpc = {
  resolve: (data: unknown) => void;
  reject: (err: Error) => void;
  timer: number;
};

type InboundQueue = { chain: Promise<void>; queued: number };
type Strikes = { since: number; count: number };
type PendingAck = { seq: number; timer: number };
type PreviewTargets = { keep: Set<string>; recent: Set<string>; changedAt: number; confirmed: boolean };
type ListName = RemotePage["list"] | "accounts";
const ALL_LISTS: ListName[] = ["accounts", "groups", "favourites", "instances", "metrics", "crashes", "bulkItems"];
type TabSlot = { abort: AbortController; release: (() => void) | null };

function inFlight(t: PreviewTargets, now: number): boolean {
  return now - t.changedAt < (t.confirmed ? PREVIEW_GRACE_MS : PREVIEW_ASK_MS);
}

/** Full-jitter exponential backoff: 0.5 s plus a random share of a cap that doubles from 2 s up to 60 s. */
export function reconnectDelay(attempt: number, random = Math.random): number {
  return Math.round(500 + random() * Math.min(60_000, 2_000 * 2 ** Math.min(attempt, 5)));
}

/** The backoff step after a socket closed that stayed open `upMs`: as the PC does, only a
 * connection that lasted `STABLE_MS` starts over, and then at a wider step, so a relay that
 * accepts and drops sockets doesn't keep every dashboard retrying every second. */
export function backoffAfterClose(attempt: number, upMs: number): number {
  return upMs >= STABLE_MS ? AFTER_STABLE_ATTEMPT : attempt;
}

function rpcErrText(code: string | null | undefined, error: string | null | undefined): string {
  if (code === "rate_limited") return "This PC is getting too many requests. Wait a moment and try again.";
  if (code === "previews_off") return "This PC has live previews turned off for View-only devices.";
  return error || code || "Command failed";
}

function randomReqId(): number {
  return crypto.getRandomValues(new Uint32Array(1))[0];
}

/** Manages 1 multiplexed WebSocket per unique relay URL across all paired PCs. */
export class RemoteManager {
  private sockets = new Map<string, WebSocket>();
  private reconnectTimers = new Map<string, number>();
  private reconnectAttempts = new Map<string, number>();
  private openedAt = new Map<string, number>();
  private pingTimers = new Map<string, number>();
  private sessions = new Map<string, SessionState>();
  private runtimes = new Map<string, PcRuntime>();
  private pendingRpcs = new Map<string, PendingRpc>();
  private pcs: PairedPc[] = [];
  private closed = false;
  private upgradeRequired = false;
  private relayIssue: RelayIssue | null = null;
  /** Each PC's relay messages run in order (its handshake or first snapshot must not race the `pc_online` that set its session up), apart from every other PC's. */
  private inbound = new Map<string, InboundQueue>();
  /** Each PC's seals and sends go out one at a time, so frames reach it in sequence-number order; PCs don't wait on each other. */
  private outbound = new Map<string, Promise<void>>();
  private strikes = new Map<string, Strikes>();
  private pendingAcks = new Map<string, PendingAck>();
  private ackBudgets = new Map<string, { tokens: number; at: number }>();
  private handshakeTimers = new Map<string, number>();
  private handshakeTries = new Map<string, number>();
  private resubscribeTimers = new Map<string, number>();
  private restarting = new Set<string>();
  private heldUntilOnline = new Set<string>();
  private listBatches = new Map<string, Partial<Record<ListName, number>>>();
  private idleRelays = new Set<string>();
  private resyncs = new Map<string, { at: number; wait: number }>();
  private resyncTimers = new Map<string, number>();
  private hiddenTimer = 0;
  private ended = new Set<string>();
  private replacedAt = new Map<string, number>();
  private listeners = new Set<() => void>();
  private emitFrame = 0;
  private frames = new Map<string, { jpeg: Blob; seq: number }>();
  private watchers = new Map<string, PreviewWatcher>();
  private previewTargets = new Map<string, PreviewTargets>();
  /** The PC lets one device in from `TABS_PER_PC` sessions at once, so this browser's tabs share
   * that many slots per PC and a tab without one never subscribes, instead of evicting another. */
  private tabSlots = new Map<string, TabSlot>();
  private livenessTimer: number;

  constructor() {
    this.livenessTimer = window.setInterval(() => this.pingSessions(), LIVENESS_PING_MS);
    document.addEventListener("visibilitychange", this.onVisibility);
  }

  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit() {
    if (this.emitFrame || this.closed) return;
    this.emitFrame = window.requestAnimationFrame(() => {
      this.emitFrame = 0;
      for (const fn of this.listeners) fn();
    });
  }

  getRuntimes(): Record<string, PcRuntime> {
    const out: Record<string, PcRuntime> = {};
    for (const [id, rt] of this.runtimes) out[id] = rt;
    return out;
  }

  isUpgradeRequired(): boolean {
    return this.upgradeRequired;
  }

  getRelayIssue(): RelayIssue | null {
    return this.relayIssue;
  }

  setPcs(pcs: PairedPc[]) {
    this.pcs = pcs;
    const next = new Map(pcs.map((p) => [p.pcId, p]));
    for (const [id, rt] of [...this.runtimes]) {
      const repaired = next.get(id);
      if (repaired?.deviceId === rt.pc.deviceId) continue;
      const ws = this.sockets.get(rt.pc.relayUrl);
      if (repaired && ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ t: "unsubscribe", pcId: id }));
      if (!repaired) this.releaseTabSlot(id);
      this.dropFrames(id);
      this.rejectPending(id);
      this.runtimes.delete(id);
      this.sessions.delete(id);
      this.inbound.delete(id);
      this.outbound.delete(id);
      this.strikes.delete(id);
      this.clearHandshakeTimer(id);
      this.clearResyncTimer(id);
      this.handshakeTries.delete(id);
      this.ended.delete(id);
      this.replacedAt.delete(id);
      this.heldUntilOnline.delete(id);
      this.listBatches.delete(id);
      this.previewTargets.delete(id);
      this.ackBudgets.delete(id);
    }
    for (const pc of pcs) {
      const existing = this.runtimes.get(pc.pcId);
      if (existing) {
        existing.pc = pc;
      } else {
        if (!this.tabSlots.has(pc.pcId)) this.claimTabSlot(pc.pcId);
        const status = this.tabSlots.get(pc.pcId)?.release === null ? "tooManyTabs" : "connecting";
        this.runtimes.set(pc.pcId, { pc, status, snapshot: null, previews: {}, lastNotice: null, problem: null, previewsOff: false });
      }
    }

    const neededRelays = new Set(pcs.map((p) => p.relayUrl));
    for (const [relay, ws] of this.sockets) {
      if (!neededRelays.has(relay)) {
        ws.close();
        this.sockets.delete(relay);
        this.clearTimers(relay);
      } else if (ws.readyState === WebSocket.OPEN) {
        this.sendSubscriptions(relay, ws);
      }
    }
    for (const relay of neededRelays) {
      if (!this.sockets.has(relay) && !this.reconnectTimers.has(relay)) this.connectRelay(relay);
    }
    this.emit();
  }

  destroy() {
    this.closed = true;
    window.cancelAnimationFrame(this.emitFrame);
    window.clearInterval(this.livenessTimer);
    window.clearTimeout(this.hiddenTimer);
    document.removeEventListener("visibilitychange", this.onVisibility);
    for (const relay of [...this.sockets.keys()]) {
      this.sockets.get(relay)?.close();
      this.clearTimers(relay);
    }
    for (const relay of [...this.reconnectTimers.keys()]) this.clearTimers(relay);
    for (const pcId of [...this.handshakeTimers.keys()]) this.clearHandshakeTimer(pcId);
    for (const pcId of [...this.resyncTimers.keys()]) this.clearResyncTimer(pcId);
    for (const pcId of [...this.tabSlots.keys()]) this.releaseTabSlot(pcId);
    for (const ack of this.pendingAcks.values()) window.clearTimeout(ack.timer);
    this.pendingAcks.clear();
    this.sockets.clear();
    this.frames.clear();
    this.watchers.clear();
    for (const pending of this.pendingRpcs.values()) {
      window.clearTimeout(pending.timer);
      pending.reject(new Error("Disconnected"));
    }
    this.pendingRpcs.clear();
  }

  private onVisibility = () => {
    if (this.closed) return;
    window.clearTimeout(this.hiddenTimer);
    if (document.hidden) {
      this.hiddenTimer = window.setTimeout(() => {
        if (!document.hidden) return;
        for (const ws of this.sockets.values()) ws.close();
        for (const pcId of [...this.tabSlots.keys()]) this.releaseTabSlot(pcId);
      }, HIDDEN_CLOSE_MS);
      return;
    }
    for (const pc of this.pcs) if (!this.tabSlots.has(pc.pcId) && !this.ended.has(pc.pcId)) this.claimTabSlot(pc.pcId);
    for (const relay of new Set(this.pcs.map((p) => p.relayUrl))) {
      const ws = this.sockets.get(relay);
      if (ws?.readyState === WebSocket.OPEN) {
        this.sendSubscriptions(relay, ws);
        continue;
      }
      if (ws) continue;
      window.clearTimeout(this.reconnectTimers.get(relay));
      this.reconnectTimers.delete(relay);
      this.connectRelay(relay);
    }
  };

  private pingSessions() {
    const now = Date.now();
    for (const [pcId, sess] of [...this.sessions]) {
      if (!sess.cipher) continue;
      if (now - sess.lastRx > SESSION_SILENCE_MS) this.sessionLost(pcId);
      else this.post(pcId, { reqId: 0, op: "ping" }).catch(() => {});
    }
  }

  private sessionLost(pcId: string) {
    const rt = this.runtimes.get(pcId);
    this.sessions.delete(pcId);
    this.rejectPending(pcId);
    this.restarting.add(pcId);
    if (!rt) return;
    rt.status = "connecting";
    this.emit();
    this.resubscribe(rt.pc.relayUrl);
    this.armRetry(pcId);
  }

  private clearTimers(relay: string) {
    const rt = this.reconnectTimers.get(relay);
    if (rt) window.clearTimeout(rt);
    this.reconnectTimers.delete(relay);
    const pt = this.pingTimers.get(relay);
    if (pt) window.clearInterval(pt);
    this.pingTimers.delete(relay);
    window.clearTimeout(this.resubscribeTimers.get(relay));
    this.resubscribeTimers.delete(relay);
  }

  private connectRelay(relay: string, delayMs = 0) {
    if (this.closed || this.upgradeRequired) return;
    if (delayMs > 0) {
      const t = window.setTimeout(() => {
        this.reconnectTimers.delete(relay);
        if (!this.sockets.has(relay) && this.pcs.some((p) => p.relayUrl === relay)) this.connectRelay(relay, 0);
      }, delayMs);
      this.reconnectTimers.set(relay, t);
      return;
    }
    if (document.hidden) return;

    const ws = new WebSocket(relay);
    ws.binaryType = "arraybuffer";
    this.sockets.set(relay, ws);

    ws.onopen = () => {
      this.openedAt.set(relay, Date.now());
      this.sendSubscriptions(relay, ws);
      const pt = window.setInterval(() => {
        if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ t: "ping" }));
      }, RELAY_PING_MS);
      this.pingTimers.set(relay, pt);
    };

    ws.onmessage = (ev) => {
      const data: unknown = ev.data;
      if (typeof data === "string") this.onText(ws, data);
      else if (data instanceof ArrayBuffer) this.onBinary(ws, new Uint8Array(data));
    };

    ws.onclose = () => {
      if (this.sockets.get(relay) !== ws) return;
      this.clearTimers(relay);
      const upMs = Date.now() - (this.openedAt.get(relay) ?? Date.now());
      const idle = this.idleRelays.delete(relay);
      this.sockets.delete(relay);
      this.openedAt.delete(relay);
      for (const pc of this.pcs.filter((p) => p.relayUrl === relay)) {
        this.sessions.delete(pc.pcId);
        this.inbound.delete(pc.pcId);
        this.heldUntilOnline.delete(pc.pcId);
        this.handshakeTries.delete(pc.pcId);
        this.clearHandshakeTimer(pc.pcId);
        this.rejectPending(pc.pcId);
        const rt = this.runtimes.get(pc.pcId);
        if (rt && rt.status !== "tooManyTabs" && rt.status !== "paused" && rt.status !== "deviceBusy") rt.status = "offline";
      }
      this.emit();
      if (!this.closed && this.pcs.some((p) => p.relayUrl === relay)) {
        const previous = this.reconnectAttempts.get(relay) ?? 0;
        const attempt = idle ? previous : backoffAfterClose(previous, upMs);
        this.reconnectAttempts.set(relay, attempt + 1);
        this.connectRelay(relay, reconnectDelay(attempt));
      }
    };
  }

  private sendSubscriptions(relay: string, ws: WebSocket) {
    const mine = this.pcs.filter((p) => p.relayUrl === relay && !this.ended.has(p.pcId) && this.tabSlots.get(p.pcId)?.release);
    ws.send(
      JSON.stringify({
        t: "subscribe",
        proto: PROTO,
        pcs: mine.map((p) => ({
          pcId: p.pcId,
          deviceId: p.deviceId,
          roomToken: p.roomToken,
          ...(this.restarting.delete(p.pcId) ? { restart: true } : {}),
        })),
      }),
    );
    for (const p of mine) if (!this.handshakeTimers.has(p.pcId) && this.runtimes.get(p.pcId)?.status !== "online") this.armRetry(p.pcId);
  }

  private claimTabSlot(pcId: string) {
    const slot: TabSlot = { abort: new AbortController(), release: null };
    this.tabSlots.set(pcId, slot);
    const locks = typeof navigator === "undefined" ? undefined : navigator.locks;
    if (!locks) {
      slot.release = () => {};
      return;
    }
    const names = Array.from({ length: TABS_PER_PC }, (_, i) => `roalt-remote/${pcId}/${i}`);
    let misses = 0;
    const take = (lock: Lock | null) => {
      if (slot.abort.signal.aborted) return;
      if (lock) {
        slot.abort.abort();
        return new Promise<void>((release) => this.tabSlotHeld(pcId, slot, release));
      }
      if (++misses < TABS_PER_PC) return;
      const rt = this.runtimes.get(pcId);
      if (rt) {
        rt.status = "tooManyTabs";
        this.emit();
      }
      for (const name of names) locks.request(name, { signal: slot.abort.signal }, take).catch(() => {});
    };
    for (const name of names) locks.request(name, { ifAvailable: true }, take).catch(() => {});
  }

  private tabSlotHeld(pcId: string, slot: TabSlot, release: () => void) {
    if (this.closed || this.tabSlots.get(pcId) !== slot) return release();
    slot.release = release;
    const rt = this.runtimes.get(pcId);
    if (!rt) return;
    if (rt.status === "tooManyTabs") {
      rt.status = "connecting";
      this.emit();
    }
    if (this.sockets.get(rt.pc.relayUrl)?.readyState === WebSocket.OPEN) this.resubscribe(rt.pc.relayUrl);
  }

  private releaseTabSlot(pcId: string) {
    const slot = this.tabSlots.get(pcId);
    this.tabSlots.delete(pcId);
    slot?.abort.abort();
    slot?.release?.();
  }

  private enqueue(pcId: string, job: () => Promise<void> | void, droppable: boolean) {
    let q = this.inbound.get(pcId);
    if (!q) {
      q = { chain: Promise.resolve(), queued: 0 };
      this.inbound.set(pcId, q);
    }
    if (droppable && q.queued >= MAX_QUEUED_FRAMES) return;
    const queue = q;
    queue.queued++;
    queue.chain = queue.chain
      .then(job)
      .catch((err) => console.error("remote: bad relay frame", err))
      .finally(() => queue.queued--);
  }

  private onText(ws: WebSocket, raw: string) {
    let msg: { t?: string; pcId?: string; code?: string };
    try {
      msg = JSON.parse(raw);
    } catch {
      return;
    }
    if (msg.t === "err") {
      if (msg.code === "no_purpose") {
        for (const [relay, socket] of this.sockets) if (socket === ws) this.idleRelays.add(relay);
        return;
      }
      this.onRelayErr(msg.code);
      return;
    }
    const pcId = msg.pcId;
    if (typeof pcId !== "string" || !this.runtimes.has(pcId)) return;
    this.enqueue(pcId, () => this.onPcText(ws, pcId, msg.t, msg.code), false);
  }

  private onRelayErr(code: string | undefined) {
    if (code === "upgrade_required") {
      this.upgradeRequired = true;
    } else if (code === "frame_too_large" || code === "rate_limited" || code === "bad_version") {
      this.relayIssue = { code, at: Date.now() };
    } else {
      return;
    }
    this.emit();
  }

  private async onPcText(ws: WebSocket, pcId: string, t: string | undefined, code: string | undefined) {
    const rt = this.runtimes.get(pcId);
    if (!rt || this.ended.has(pcId)) return;

    if (t === "pc_online") {
      this.heldUntilOnline.delete(pcId);
      if (this.sessions.has(pcId)) return;
      if (rt.status !== "deviceBusy") rt.status = "connecting";
      rt.problem = null;
      rt.previewsOff = false;
      this.emit();
      const sess = await this.newSession(rt.pc);
      if (this.sockets.get(rt.pc.relayUrl) !== ws || this.sessions.has(pcId) || this.ended.has(pcId)) return;
      this.sessions.set(pcId, sess);
      this.sendBinary(ws, pcId, sess.handshake);
      this.armRetry(pcId);
    } else if (t === "pc_offline") {
      this.sessions.delete(pcId);
      this.rejectPending(pcId);
      if (rt.status !== "paused") rt.status = "offline";
      this.emit();
      this.armRetry(pcId);
    } else if (t === "pc_err" && code === "full") {
      this.sessions.delete(pcId);
      this.clearHandshakeTimer(pcId);
      this.rejectPending(pcId);
      rt.status = "offline";
      rt.problem = PC_FULL;
      this.emit();
    } else if (t === "pc_err" && code === "too_many") {
      this.sessions.delete(pcId);
      this.clearHandshakeTimer(pcId);
      this.rejectPending(pcId);
      rt.status = "tooMany";
      this.emit();
    } else if (t === "pc_err" && (code === "paused" || code === "device_sessions")) {
      this.sessions.delete(pcId);
      this.rejectPending(pcId);
      rt.status = code === "paused" ? "paused" : "deviceBusy";
      rt.problem = null;
      if (code === "paused") this.heldUntilOnline.add(pcId);
      this.armRetry(pcId);
      this.emit();
    } else if (t === "pc_err" && code === "replaced" && this.sockets.get(rt.pc.relayUrl) === ws) {
      const now = Date.now();
      const last = this.replacedAt.get(pcId);
      this.replacedAt.set(pcId, now);
      if (last !== undefined && now - last < REPLACED_AGAIN_MS) {
        this.endSession(pcId, REPLACED);
        return;
      }
      this.sessions.delete(pcId);
      this.rejectPending(pcId);
      rt.status = "connecting";
      this.armRetry(pcId);
      this.emit();
    }
  }

  private async newSession(pc: PairedPc): Promise<SessionState> {
    const ephem = await generateEphemeralEcdh();
    const handshake = await buildHandshake(pc.keyPair.privateKey, ephem.publicKeyRaw, DIR_VIEWER_TO_PC, pc.pcId, pc.deviceId);
    return { pc, ephemPriv: ephem.keyPair.privateKey, ephemPubRaw: ephem.publicKeyRaw, handshake, cipher: null, badEvents: { since: 0, count: 0 }, lastRx: Date.now() };
  }

  private armRetry(pcId: string) {
    this.clearHandshakeTimer(pcId);
    if (this.heldUntilOnline.has(pcId)) return;
    const tries = this.handshakeTries.get(pcId) ?? 0;
    const timer = window.setTimeout(() => {
      this.handshakeTimers.delete(pcId);
      const rt = this.runtimes.get(pcId);
      if (!rt || rt.status === "online" || rt.status === "tooMany" || this.ended.has(pcId) || document.hidden) return;
      const ws = this.sockets.get(rt.pc.relayUrl);
      if (ws?.readyState !== WebSocket.OPEN) return;
      this.handshakeTries.set(pcId, tries + 1);
      const sess = this.sessions.get(pcId);
      if (sess?.cipher) {
        this.sendBinary(ws, pcId, sess.handshake);
        this.post(pcId, { reqId: 0, op: "requestSnapshot" }).catch(() => {});
      } else {
        this.sessions.delete(pcId);
        this.restarting.add(pcId);
        this.resubscribe(rt.pc.relayUrl);
      }
      this.armRetry(pcId);
    }, HANDSHAKE_WAIT_MS * 2 ** Math.min(tries, 3));
    this.handshakeTimers.set(pcId, timer);
  }

  private resubscribe(relay: string) {
    if (this.resubscribeTimers.has(relay)) return;
    const timer = window.setTimeout(() => {
      this.resubscribeTimers.delete(relay);
      const ws = this.sockets.get(relay);
      if (ws?.readyState === WebSocket.OPEN && !document.hidden) this.sendSubscriptions(relay, ws);
    }, RESUBSCRIBE_DELAY_MS);
    this.resubscribeTimers.set(relay, timer);
  }

  private clearHandshakeTimer(pcId: string) {
    const timer = this.handshakeTimers.get(pcId);
    if (timer) window.clearTimeout(timer);
    this.handshakeTimers.delete(pcId);
  }

  /** Counts a frame the PC should not have sent; too many in a short time end its session. */
  private strike(pcId: string) {
    const now = Date.now();
    const s = this.strikes.get(pcId);
    const cur = s && now - s.since < STRIKE_WINDOW_MS ? s : { since: now, count: 0 };
    cur.count++;
    this.strikes.set(pcId, cur);
    if (cur.count <= MAX_STRIKES) return;
    this.strikes.delete(pcId);
    this.endSession(pcId, MISBEHAVING);
  }

  private endSession(pcId: string, problem: string) {
    const rt = this.runtimes.get(pcId);
    const ws = rt && this.sockets.get(rt.pc.relayUrl);
    this.ended.add(pcId);
    this.sessions.delete(pcId);
    this.clearHandshakeTimer(pcId);
    this.rejectPending(pcId);
    if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ t: "unsubscribe", pcId }));
    this.releaseTabSlot(pcId);
    if (rt) {
      rt.status = "offline";
      rt.problem = problem;
    }
    this.emit();
  }

  private rejectPending(pcId: string) {
    const prefix = `${pcId}:`;
    for (const [key, p] of this.pendingRpcs) {
      if (!key.startsWith(prefix)) continue;
      window.clearTimeout(p.timer);
      this.pendingRpcs.delete(key);
      p.reject(new Error("PC disconnected"));
    }
  }

  private sendBinary(ws: WebSocket, pcId: string, payload: Uint8Array) {
    if (ws.readyState !== WebSocket.OPEN) return;
    const frame = concat(new Uint8Array([BIN_VERSION]), uuidToBytes(pcId), payload);
    ws.send(frame);
  }

  private onBinary(_ws: WebSocket, bytes: Uint8Array) {
    if (bytes.length < BIN_HEADER_LEN + 1 || bytes[0] !== BIN_VERSION) return;
    const pcId = bytesToUuid(bytes.slice(1, 17));
    if (!this.runtimes.has(pcId)) return;
    const payload = bytes.slice(BIN_HEADER_LEN);
    this.enqueue(pcId, () => this.onPcFrame(pcId, payload), true);
  }

  private async onPcFrame(pcId: string, payload: Uint8Array) {
    const sess = this.sessions.get(pcId);
    const rt = this.runtimes.get(pcId);
    if (!sess || !rt) return;

    if (payload[0] === FRAME_HANDSHAKE && sess.cipher) {
      await this.rekey(pcId, sess, payload);
      return;
    }

    if (payload[0] === FRAME_HANDSHAKE) {
      try {
        const pcStaticPub = fromB64Url(sess.pc.pcPubB64);
        const pcEphemPub = await verifyHandshake(pcStaticPub, payload, DIR_PC_TO_VIEWER, pcId, sess.pc.deviceId);
        sess.cipher = await SessionCipher.derive(sess.ephemPriv, pcEphemPub, pcEphemPub, sess.ephemPubRaw, false);
        sess.lastRx = Date.now();
      } catch (err) {
        console.error("E2EE handshake verification failed for PC", pcId, err);
        this.endSession(pcId, BAD_HANDSHAKE);
        return;
      }
      this.post(pcId, { reqId: 0, op: "ping" }).catch(() => {});
      return;
    }

    if (payload[0] === FRAME_DATA && sess.cipher) {
      let pt: Uint8Array;
      try {
        pt = await sess.cipher.open(payload);
      } catch {
        this.strike(pcId);
        return;
      }
      sess.lastRx = Date.now();
      if (pt.length < 2) {
        this.strike(pcId);
        return;
      }
      const kind = pt[0];
      if (kind === APP_KIND_JSON) {
        let raw: unknown = null;
        try {
          raw = JSON.parse(dec.decode(pt.slice(1)));
        } catch {
          raw = null;
        }
        const ev = parseEvent(raw);
        if (ev) this.handleEvent(rt, ev);
        else if (isUnknownEvent(raw)) console.debug("remote: ignored an event type this dashboard doesn't know", pcId);
        else {
          console.warn("remote: dropped a malformed event from PC", pcId);
          const now = Date.now();
          if (now - sess.badEvents.since >= BAD_EVENT_WINDOW_MS) sess.badEvents = { since: now, count: 0 };
          if (++sess.badEvents.count >= MAX_BAD_EVENTS) return this.endSession(pcId, BAD_DATA);
          rt.problem = BAD_DATA;
          this.strike(pcId);
          this.emit();
        }
      } else if (kind === APP_KIND_PREVIEW) {
        this.handlePreview(rt, pt);
      } else {
        this.strike(pcId);
      }
    } else {
      this.strike(pcId);
    }
  }

  private async rekey(pcId: string, sess: SessionState, payload: Uint8Array) {
    let pcEphemPub: Uint8Array;
    try {
      pcEphemPub = await verifyHandshake(fromB64Url(sess.pc.pcPubB64), payload, DIR_PC_TO_VIEWER, pcId, sess.pc.deviceId);
    } catch {
      this.strike(pcId);
      return;
    }
    this.strike(pcId);
    const rt = this.runtimes.get(pcId);
    const ws = this.sockets.get(sess.pc.relayUrl);
    if (!rt || !ws || this.sessions.get(pcId) !== sess) return;
    const next = await this.newSession(sess.pc);
    next.cipher = await SessionCipher.derive(next.ephemPriv, pcEphemPub, pcEphemPub, next.ephemPubRaw, false);
    if (this.sessions.get(pcId) !== sess) return;
    this.sessions.set(pcId, next);
    this.rejectPending(pcId);
    rt.status = "connecting";
    this.emit();
    this.sendBinary(ws, pcId, next.handshake);
    this.post(pcId, { reqId: 0, op: "ping" }).catch(() => {});
    this.armRetry(pcId);
  }

  private handleEvent(rt: PcRuntime, ev: RemoteEvent) {
    switch (ev.t) {
      case "snapshot":
        rt.snapshot = { ...ev.data, pcName: [...cleanName(ev.data.pcName)].slice(0, MAX_PC_NAME).join("") };
        this.setBatch(rt, ev.batch, ALL_LISTS);
        rt.status = "online";
        rt.problem = null;
        rt.lastNotice = null;
        this.clearResyncTimer(rt.pc.pcId);
        this.clearHandshakeTimer(rt.pc.pcId);
        this.handshakeTries.delete(rt.pc.pcId);
        break;
      case "accountsUpdated":
        if (rt.snapshot) {
          rt.snapshot = { ...rt.snapshot, accounts: ev.accounts, groups: ev.groups };
          this.setBatch(rt, ev.batch, ["accounts", "groups"]);
        }
        break;
      case "accountsPage": {
        if (!rt.snapshot) return;
        const accounts = this.ofBatch(rt, "accounts", ev.batch) && this.appendPage(rt, rt.snapshot.accounts, ev.offset, ev.accounts);
        if (!accounts) return this.resync(rt);
        rt.snapshot = { ...rt.snapshot, accounts };
        break;
      }
      case "page": {
        if (!rt.snapshot) return;
        const next = this.ofBatch(rt, ev.page.list, ev.batch) && this.withPage(rt, rt.snapshot, ev.offset, ev.page);
        if (!next) return this.resync(rt);
        rt.snapshot = next;
        break;
      }
      case "instancesUpdated":
        if (rt.snapshot) {
          rt.snapshot = { ...rt.snapshot, instances: ev.instances };
          this.setBatch(rt, ev.batch, ["instances"]);
        }
        break;
      case "metricsUpdated":
        if (rt.snapshot) {
          this.setBatch(rt, ev.batch, ["metrics"]);
          rt.snapshot = {
            ...rt.snapshot,
            metrics: ev.metrics,
            systemUsage: ev.systemUsage,
            windowsLocked: ev.windowsLocked,
            vaultLocked: ev.vaultLocked,
            shutdownAt: ev.shutdownAt,
          };
        }
        break;
      case "bulkProgress":
        if (rt.snapshot) {
          rt.snapshot = { ...rt.snapshot, bulkProgress: ev.progress };
          this.setBatch(rt, ev.batch, ["bulkItems"]);
        }
        break;
      case "notice":
        rt.lastNotice = { error: ev.error, message: ev.message, at: Date.now() };
        if (ev.error && !rt.snapshot) rt.problem = ev.message;
        break;
      case "rpcResult": {
        const key = `${rt.pc.pcId}:${ev.reqId}`;
        const p = this.pendingRpcs.get(key);
        if (p) {
          window.clearTimeout(p.timer);
          this.pendingRpcs.delete(key);
          if (ev.ok) p.resolve(ev.data);
          else p.reject(new RpcError(rpcErrText(ev.code, ev.error), ev.code ?? null));
        }
        return;
      }
      case "webRtcSignal":
        return;
    }
    this.emit();
  }

  private setBatch(rt: PcRuntime, batch: number, lists: ListName[]) {
    const batches = this.listBatches.get(rt.pc.pcId) ?? {};
    for (const list of lists) batches[list] = batch;
    this.listBatches.set(rt.pc.pcId, batches);
  }

  private ofBatch(rt: PcRuntime, list: ListName, batch: number): boolean {
    return this.listBatches.get(rt.pc.pcId)?.[list] === batch;
  }

  private appendPage<T>(rt: PcRuntime, list: T[], offset: number, items: T[]): T[] | null {
    const cut = () => (rt.lastNotice = { error: true, message: LIST_TOO_LONG, at: Date.now() });
    if (list.length >= MAX_LIST_ITEMS && offset >= MAX_LIST_ITEMS) {
      cut();
      return list;
    }
    if (offset !== list.length) return null;
    if (list.length + items.length > MAX_LIST_ITEMS) {
      cut();
      return [...list, ...items.slice(0, MAX_LIST_ITEMS - list.length)];
    }
    return [...list, ...items];
  }

  private resync(rt: PcRuntime) {
    const pcId = rt.pc.pcId;
    rt.lastNotice = { error: true, message: LIST_GAP, at: Date.now() };
    this.emit();
    if (this.resyncTimers.has(pcId)) return;
    const last = this.resyncs.get(pcId);
    const due = last ? last.at + last.wait - Date.now() : 0;
    if (due <= 0) return this.requestSnapshot(pcId);
    const timer = window.setTimeout(() => {
      this.resyncTimers.delete(pcId);
      this.requestSnapshot(pcId);
    }, due);
    this.resyncTimers.set(pcId, timer);
  }

  private requestSnapshot(pcId: string) {
    const now = Date.now();
    const last = this.resyncs.get(pcId);
    const wait = last && now - last.at < RESYNC_MAX_MS * 2 ? Math.min(last.wait * 2, RESYNC_MAX_MS) : RESYNC_MIN_MS;
    this.resyncs.set(pcId, { at: now, wait });
    this.post(pcId, { reqId: 0, op: "requestSnapshot" }).catch(() => {});
  }

  private clearResyncTimer(pcId: string) {
    window.clearTimeout(this.resyncTimers.get(pcId));
    this.resyncTimers.delete(pcId);
  }

  private withPage(rt: PcRuntime, snap: RemoteSnapshot, offset: number, page: RemotePage): RemoteSnapshot | null {
    switch (page.list) {
      case "groups": {
        const groups = this.appendPage(rt, snap.groups, offset, page.items);
        return groups && { ...snap, groups };
      }
      case "favourites": {
        const favourites = this.appendPage(rt, snap.favourites, offset, page.items);
        return favourites && { ...snap, favourites };
      }
      case "instances": {
        const instances = this.appendPage(rt, snap.instances, offset, page.items);
        return instances && { ...snap, instances };
      }
      case "metrics": {
        const metrics = this.appendPage(rt, snap.metrics, offset, page.items);
        return metrics && { ...snap, metrics };
      }
      case "crashes": {
        const crashes = this.appendPage(rt, snap.crashes, offset, page.items);
        return crashes && { ...snap, crashes };
      }
      case "bulkItems": {
        const bulk = snap.bulkProgress;
        const items = bulk && this.appendPage(rt, bulk.items, offset, page.items);
        return bulk && items && { ...snap, bulkProgress: { ...bulk, items } };
      }
    }
  }

  private handlePreview(rt: PcRuntime, pt: Uint8Array) {
    const idLen = pt[1];
    const headerEnd = 2 + idLen + 9;
    if (pt.length < 11 || pt.length < headerEnd) {
      this.strike(rt.pc.pcId);
      return;
    }
    const instanceId = dec.decode(pt.slice(2, 2 + idLen));
    const asked = this.previewTargets.get(rt.pc.pcId);
    if (!asked?.keep.has(instanceId)) {
      if (!asked?.recent.has(instanceId) || !inFlight(asked, Date.now())) this.strike(rt.pc.pcId);
      return;
    }
    const dv = new DataView(pt.buffer, pt.byteOffset + 2 + idLen, 9);
    const seq = dv.getUint32(0, true);
    const ageMs = dv.getUint32(4, true);
    const status = dv.getUint8(8);
    const jpeg = pt.slice(headerEnd);

    const key = `${rt.pc.pcId}/${instanceId}`;
    const prev = rt.previews[instanceId];
    // A frame without a picture only updates the status; a minimized window keeps its last picture.
    const picture = jpeg.length > 0 || (status === 1 && !!prev?.picture);
    if (jpeg.length === 0 && !picture) this.frames.delete(key);
    if (!prev || prev.status !== status || prev.picture !== picture) {
      rt.previews = { ...rt.previews, [instanceId]: { instanceId, seq, ageMs, status, picture } };
      this.emit();
    }
    if (jpeg.length === 0) return;
    const frame = { jpeg: new Blob([jpeg], { type: "image/jpeg" }), seq };
    this.frames.set(key, frame);
    const watcher = this.watchers.get(key);
    if (!watcher) {
      this.ackPreview(rt.pc.pcId, instanceId, seq);
      return;
    }
    watcher.next = frame;
    void this.drainPreview(rt.pc.pcId, instanceId, watcher);
  }

  private async drainPreview(pcId: string, instanceId: string, watcher: PreviewWatcher) {
    if (watcher.busy) return;
    watcher.busy = true;
    while (watcher.next) {
      const { jpeg, seq } = watcher.next;
      watcher.next = null;
      await watcher.draw(jpeg).catch(() => {});
      this.ackPreview(pcId, instanceId, seq);
    }
    watcher.busy = false;
  }

  /** Acks each picture right away while the PC's `PREVIEW_ACKS_PER_SEC` budget lasts (the PC never
   * sends more pictures than that), else once a token frees up, for the newest picture (it covers
   * the older ones), so a PC flooding tiny frames can't make this tab flood the relay with acks. */
  private ackPreview(pcId: string, instanceId: string, seq: number) {
    const key = `${pcId}/${instanceId}`;
    const pending = this.pendingAcks.get(key);
    if (pending) {
      pending.seq = Math.max(pending.seq, seq);
      return;
    }
    const wait = this.ackWait(pcId);
    if (wait === 0) {
      this.post(pcId, { reqId: 0, op: "previewAck", instanceId, seq }).catch(() => {});
      return;
    }
    const ack: PendingAck = {
      seq,
      timer: window.setTimeout(() => {
        this.pendingAcks.delete(key);
        this.ackPreview(pcId, instanceId, ack.seq);
      }, wait),
    };
    this.pendingAcks.set(key, ack);
  }

  /** Takes an ack token for `pcId` (0) or says how many ms until one is free. */
  private ackWait(pcId: string): number {
    const now = Date.now();
    const b = this.ackBudgets.get(pcId) ?? { tokens: PREVIEW_ACKS_PER_SEC, at: now };
    b.tokens = Math.min(PREVIEW_ACKS_PER_SEC, b.tokens + ((now - b.at) / 1000) * PREVIEW_ACKS_PER_SEC);
    b.at = now;
    this.ackBudgets.set(pcId, b);
    if (b.tokens < 1) return Math.max(1, Math.ceil(((1 - b.tokens) / PREVIEW_ACKS_PER_SEC) * 1000));
    b.tokens -= 1;
    return 0;
  }

  private dropFrames(pcId: string) {
    for (const key of this.frames.keys()) if (key.startsWith(`${pcId}/`)) this.frames.delete(key);
  }

  /** Calls `draw` with each new picture of the instance (the newest one first, if any); returns the unsubscribe. */
  watchPreview(pcId: string, instanceId: string, draw: (jpeg: Blob) => Promise<void>): () => void {
    const key = `${pcId}/${instanceId}`;
    const watcher: PreviewWatcher = { draw, busy: false, next: null };
    this.watchers.set(key, watcher);
    const last = this.frames.get(key);
    if (last) void draw(last.jpeg).catch(() => {});
    return () => {
      if (this.watchers.get(key) === watcher) this.watchers.delete(key);
    };
  }

  /** Sends an encrypted RPC command to `pcId` and awaits its `RpcResult` from that same PC. */
  async rpc<T = unknown>(pcId: string, cmd: Record<string, unknown>, timeoutMs = 15_000): Promise<T> {
    this.channel(pcId);
    let reqId = randomReqId();
    while (reqId === 0 || this.pendingRpcs.has(`${pcId}:${reqId}`)) reqId = randomReqId();
    const key = `${pcId}:${reqId}`;
    const result = new Promise<T>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        this.pendingRpcs.delete(key);
        reject(new Error("Command timed out"));
      }, timeoutMs);
      this.pendingRpcs.set(key, { resolve: resolve as (d: unknown) => void, reject, timer });
    });
    try {
      await this.post(pcId, { reqId, ...cmd });
    } catch (err) {
      const p = this.pendingRpcs.get(key);
      if (p) window.clearTimeout(p.timer);
      this.pendingRpcs.delete(key);
      throw err;
    }
    return result;
  }

  private channel(pcId: string): { ws: WebSocket; cipher: SessionCipher } {
    const sess = this.sessions.get(pcId);
    const cipher = sess?.cipher;
    if (!sess || !cipher) throw new Error("PC is not connected");
    const ws = this.sockets.get(sess.pc.relayUrl);
    if (!ws || ws.readyState !== WebSocket.OPEN) throw new Error("Relay offline");
    return { ws, cipher };
  }

  private async post(pcId: string, body: Record<string, unknown>): Promise<void> {
    const { ws, cipher } = this.channel(pcId);
    const payload = concat(new Uint8Array([APP_KIND_JSON]), enc.encode(JSON.stringify(body)));
    const sent = (this.outbound.get(pcId) ?? Promise.resolve()).then(async () => this.sendBinary(ws, pcId, await cipher.seal(payload)));
    this.outbound.set(
      pcId,
      sent.catch(() => {}),
    );
    await sent;
  }

  /** Updates live preview subscriptions for a PC (retried until the PC confirms them); frames for
   * any other instance are dropped. Those of instances asked for before are struck once they are
   * no longer in flight (`inFlight`): asking again before the PC confirmed keeps the first ask's time. */
  setPreviewTargets(pcId: string, targets: RemotePreviewTarget[]) {
    const keep = new Set(targets.map((t) => t.instanceId));
    const prev = this.previewTargets.get(pcId);
    const now = Date.now();
    const recent = new Set(prev?.keep);
    if (prev && inFlight(prev, now)) for (const id of prev.recent) recent.add(id);
    const asked: PreviewTargets = { keep, recent, changedAt: prev && !prev.confirmed ? prev.changedAt : now, confirmed: false };
    this.previewTargets.set(pcId, asked);
    const rt = this.runtimes.get(pcId);
    if (rt) {
      const next: Record<string, PreviewFrame> = {};
      for (const [id, p] of Object.entries(rt.previews)) {
        if (keep.has(id)) next[id] = p;
        else this.frames.delete(`${pcId}/${id}`);
      }
      rt.previews = next;
    }
    this.askPreviews(pcId, targets, asked, 0);
  }

  private askPreviews(pcId: string, targets: RemotePreviewTarget[], asked: PreviewTargets, attempt: number) {
    const current = () => this.previewTargets.get(pcId) === asked;
    const confirm = () => {
      asked.confirmed = true;
      asked.changedAt = Date.now();
    };
    this.rpc(pcId, { op: "subscribePreviews", targets }, PREVIEW_RPC_MS).then(
      () => current() && confirm(),
      (err: unknown) => {
        if (!current()) return;
        const rt = this.runtimes.get(pcId);
        if (err instanceof RpcError && err.code === "previews_off") {
          confirm();
          if (rt && !rt.previewsOff) {
            rt.previewsOff = true;
            this.emit();
          }
          return;
        }
        if (attempt + 1 >= PREVIEW_RETRIES || !this.sessions.get(pcId)?.cipher) return confirm();
        window.setTimeout(() => current() && this.askPreviews(pcId, targets, asked, attempt + 1), 1_000 * 2 ** attempt);
      },
    );
  }
}

function isUnknownEvent(raw: unknown): boolean {
  const t = typeof raw === "object" && raw !== null ? (raw as { t?: unknown }).t : undefined;
  return typeof t === "string" && !KNOWN_EVENTS.has(t);
}

const PAIR_TIMEOUT_MS = 120_000;
const PAIR_CONFIRM_MS = 60_000;
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const PAIR_ID_RE = /^[0-9a-f]{16,64}$/;
const ROOM_TOKEN_RE = /^[A-Za-z0-9_-]{43}$/;
const B64URL_RE = /^[A-Za-z0-9_-]+$/;
const EMPTY = new Uint8Array();
const CHECK_FAILED = "Pairing check failed. Start a fresh pairing in RoALT.";
const MAX_PC_NAME = 64;

type PairOuter = { mode?: unknown; pairId?: unknown; pcEphemPubB64?: unknown; blob?: unknown };
type PairJoin = (pairId: string, msg: Record<string, unknown>) => void;
/** A verified ack, and the message that accepts it. */
type PairAccepted = { pc: PairedPc; pairId: string; pairKey: CryptoKey; done: () => Promise<Record<string, unknown>> };
export type PairFlow = {
  first: { pairId: string; msg: Record<string, unknown> };
  step: (outer: PairOuter, join: PairJoin) => Promise<PairAccepted | null>;
};
/** A PC whose ack checked out. Nothing is stored and the PC enrolls nothing until `confirm`, which
 * the dashboard calls only after the user said yes (for a manual code: that both codes match). */
export type PairOffer = { pc: PairedPc; sas: string | null; confirm: () => Promise<PairedPc> };
export type PairTask = { promise: Promise<PairOffer>; cancel: () => void };

type PairTexts = { notFound: string; rateLimited: string };

const PAIR_CLOSED = "RoALT closed this pairing before it finished. Start a fresh pairing in RoALT.";
const PAIR_EXPIRED = "Pairing code expired. Start a fresh pairing in RoALT.";
const QR_SECRET_RE = /^[A-Za-z0-9_-]{43}$/;

function pairErrText(code: string | undefined, texts: PairTexts): string {
  if (code === "rate_limited") return texts.rateLimited;
  if (code === "expired") return PAIR_EXPIRED;
  if (code === "closed") return PAIR_CLOSED;
  if (code === "busy") return "Another device is already using this pairing code. Start a fresh pairing in RoALT.";
  return texts.notFound;
}

/** Validates the PC's sealed ack and its signature over this pairing's transcript before anything is stored. */
export async function readPairAck(
  pairKey: CryptoKey,
  pairId: string,
  blob: string,
  transcript: Uint8Array,
  device: DeviceIdentity,
  relayUrl: string,
): Promise<PairedPc> {
  const ack = JSON.parse(dec.decode(await openPairBlob(pairKey, DIR_PC_TO_VIEWER, pairId, blob))) as Record<string, unknown>;
  const str = (v: unknown) => (typeof v === "string" ? v : "");
  const pcId = str(ack.pcId);
  const pcName = cleanName(str(ack.pcName));
  const pcPubB64 = str(ack.pcPubB64);
  const roomToken = str(ack.roomToken);
  const sigB64 = str(ack.sigB64);
  if (!UUID_RE.test(pcId) || !pcName || [...pcName].length > MAX_PC_NAME || !ROOM_TOKEN_RE.test(roomToken) || !B64URL_RE.test(pcPubB64) || !B64URL_RE.test(sigB64)) {
    throw new Error("Malformed pairing ack");
  }
  if (!(await verifyPcBinding(fromB64Url(pcPubB64), transcript, pcId, device.deviceId, fromB64Url(sigB64)))) {
    throw new Error("PC signature invalid");
  }
  return {
    pcId,
    pcName,
    pcPubB64,
    roomToken,
    relayUrl,
    role: ack.role === "control" ? "control" : "readonly",
    addedAt: Date.now(),
    deviceId: device.deviceId,
    keyPair: device.keyPair,
  };
}

/** Whether `blob` is the PC's `{mode:"enrolled"}` sealed under this pairing's key (`enrolled_msg` in
 * mod.rs): only that saves the pairing, so the relay can't make this browser replace a working one. */
export async function isEnrolledBlob(pairKey: CryptoKey, pairId: string, blob: unknown): Promise<boolean> {
  if (typeof blob !== "string") return false;
  try {
    const inner = JSON.parse(dec.decode(await openPairBlob(pairKey, DIR_PC_TO_VIEWER, pairId, blob))) as { mode?: unknown } | null;
    return inner?.mode === "enrolled";
  } catch {
    return false;
  }
}

async function sealDeviceBinding(pairKey: CryptoKey, pairId: string, transcript: Uint8Array, device: DeviceIdentity, label: string): Promise<string> {
  const sig = await signDeviceBinding(device.keyPair.privateKey, transcript, device.deviceId, device.publicKeyRaw);
  const body = { deviceId: device.deviceId, devicePubB64: toB64Url(device.publicKeyRaw), label, sigB64: toB64Url(sig) };
  return sealPairBlob(pairKey, DIR_VIEWER_TO_PC, pairId, enc.encode(JSON.stringify(body)));
}

/** The `done` message accepting `pc`. Re-pairing a PC this browser already has, it carries each old
 * device's signature over this transcript, so the PC drops those devices instead of keeping dead ones. */
async function doneMessage(pairKey: CryptoKey, pairId: string, transcript: Uint8Array, pc: PairedPc): Promise<Record<string, unknown>> {
  const stored = await loadPairedPcs();
  const refusal = whyNotStored(stored, pc);
  if (refusal) throw new Error(refusal);
  const proof: Record<string, unknown> = { mode: "done" };
  try {
    proof.replaces = await Promise.all(
      devicesReplacedBy(stored, pc).map(async (old) => {
        const oldPub = new Uint8Array(await crypto.subtle.exportKey("raw", old.keyPair.publicKey));
        const sig = await signDeviceBinding(old.keyPair.privateKey, transcript, old.deviceId, oldPub);
        return { deviceId: old.deviceId, sigB64: toB64Url(sig) };
      }),
    );
  } catch (err) {
    console.warn("remote: could not prove the old pairings; the PC keeps listing them", err);
  }
  return { mode: "done", blob: await sealPairBlob(pairKey, DIR_VIEWER_TO_PC, pairId, enc.encode(JSON.stringify(proof))) };
}

function pairOverRelay(relayUrl: string, texts: PairTexts, prepare: () => Promise<PairFlow>, sasOf: () => string | null): PairTask {
  let finish: ((err: Error) => void) | null = null;
  let cancelled = false;

  const promise = prepare().then(
    (flow) =>
      new Promise<PairOffer>((resolve, reject) => {
        if (cancelled) return reject(new Error("Cancelled"));
        const ws = new WebSocket(relayUrl);
        let done = false;
        let offered: PairAccepted | null = null;
        let enrolled: { resolve: (pc: PairedPc) => void; reject: (err: Error) => void } | null = null;
        let failure: Error | null = null;
        let chain = Promise.resolve();
        const join: PairJoin = (pairId, msg) => {
          if (ws.readyState !== WebSocket.OPEN) return;
          ws.send(JSON.stringify({ t: "pair_join", proto: PROTO, pairId, data: toB64Url(enc.encode(JSON.stringify(msg))) }));
        };
        const end = (err: Error | null, pc?: PairedPc) => {
          if (done) return;
          done = true;
          window.clearTimeout(timeout);
          window.clearInterval(keepalive);
          ws.close();
          failure = err;
          if (!offered) reject(err ?? new Error(CHECK_FAILED));
          else if (enrolled && pc) enrolled.resolve(pc);
          else if (enrolled) enrolled.reject(err ?? new Error(CHECK_FAILED));
        };
        finish = end;
        const timedOut = () => end(new Error("Pairing timed out. Start a fresh pairing in RoALT."));
        let deadline = Date.now() + PAIR_TIMEOUT_MS;
        let timeout = window.setTimeout(timedOut, PAIR_TIMEOUT_MS);
        const keepalive = window.setInterval(() => {
          if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ t: "ping" }));
        }, RELAY_PING_MS);

        const confirm = () =>
          new Promise<PairedPc>((res, rej) => {
            if (done || !offered) return rej(failure ?? new Error(CHECK_FAILED));
            if (enrolled) return rej(new Error("Already confirming"));
            enrolled = { resolve: res, reject: rej };
            const accepted = offered;
            void accepted.done().then(
              (msg) => join(accepted.pairId, msg),
              (err: unknown) => end(err instanceof Error ? err : new Error(CHECK_FAILED)),
            );
          });

        const onRelayText = async (raw: string) => {
          const msg = JSON.parse(raw) as { t?: string; data?: unknown; code?: string };
          if (msg.t === "pair_err") return end(new Error(pairErrText(msg.code, texts)));
          if (msg.t === "err" && msg.code === "upgrade_required") return end(new Error("This dashboard is out of date. Reload the page to update it."));
          if (msg.t !== "pair_msg" || typeof msg.data !== "string") return;
          const outer = JSON.parse(dec.decode(fromB64Url(msg.data))) as PairOuter;
          if (outer.mode === "denied") return end(new Error("Pairing was denied on the PC."));
          if (outer.mode === "closed") return end(new Error(PAIR_CLOSED));
          if (outer.mode === "expired") return end(new Error(PAIR_EXPIRED));
          if (outer.mode === "aborted") {
            return end(
              new Error("RoALT stopped this pairing: another device used the code, or the request was changed on the way. Start a fresh pairing in RoALT."),
            );
          }
          if (outer.mode === "enrolled") {
            if (!offered || !enrolled) return;
            if (await isEnrolledBlob(offered.pairKey, offered.pairId, outer.blob)) end(null, offered.pc);
            else end(new Error(CHECK_FAILED));
            return;
          }
          if (offered) return;
          const accepted = await flow.step(outer, join);
          if (accepted) {
            offered = accepted;
            deadline = Math.max(deadline, Date.now() + PAIR_CONFIRM_MS);
            window.clearTimeout(timeout);
            timeout = window.setTimeout(timedOut, deadline - Date.now());
            resolve({ pc: accepted.pc, sas: sasOf(), confirm });
          }
        };

        ws.onopen = () => join(flow.first.pairId, flow.first.msg);
        ws.onerror = () => end(new Error(`Could not reach relay (${relayUrl}).`));
        ws.onclose = () => end(new Error("Lost the connection to the relay. Try again."));
        ws.onmessage = (ev) => {
          if (typeof ev.data !== "string") return;
          const raw = ev.data;
          chain = chain.then(() => onRelayText(raw)).catch(() => end(new Error(CHECK_FAILED)));
        };
      }),
  );

  return {
    promise,
    cancel: () => {
      cancelled = true;
      finish?.(new Error("Cancelled"));
    },
  };
}

/** The QR flow's messages: it answers only an ack sealed under the QR key and signed over its own
 * transcript, never a manual-code challenge. The pairId must be a real one (never a manual code). */
export async function qrPairFlow(relayUrl: string, pairId: string, secretB64: string, label: string): Promise<PairFlow> {
  if (!PAIR_ID_RE.test(pairId) || !QR_SECRET_RE.test(secretB64)) throw new Error("This QR link is not valid. Open a fresh QR code in RoALT.");
  const pairKey = await deriveQrPairKey(fromB64Url(secretB64), pairId);
  const device = await newDeviceIdentity();
  const nonce = crypto.getRandomValues(new Uint8Array(32));
  const transcript = await pairTranscript(PAIR_MODE_QR, pairId, EMPTY, EMPTY, nonce);
  const blob = await sealDeviceBinding(pairKey, pairId, transcript, device, label);
  return {
    first: { pairId, msg: { mode: "qr", nonceB64: toB64Url(nonce), blob } },
    step: async (outer) => {
      if (outer.mode !== "ack" || typeof outer.blob !== "string") return null;
      const pc = await readPairAck(pairKey, pairId, outer.blob, transcript, device, relayUrl);
      return { pc, pairId, pairKey, done: () => doneMessage(pairKey, pairId, transcript, pc) };
    },
  };
}

/** QR-code pairing (`#pair=<pairId>.<secretB64>`): the relay never sees the secret, and the PC still has to approve. */
export function pairWithQr(relayUrl: string, pairId: string, secretB64: string, label: string): PairTask {
  return pairOverRelay(
    relayUrl,
    { notFound: "Pairing session not found. Open a fresh QR code in RoALT.", rateLimited: "Too many attempts. Wait a minute and try again." },
    () => qrPairFlow(relayUrl, pairId, secretB64, label),
    () => null,
  );
}

/**
 * Manual 8-char code pairing (`ABCD-1234`): commits to the browser's ephemeral key before the PC
 * reveals its own, then shows the 6-digit SAS (over the full transcript) for the user to compare.
 * The ack only becomes an offer: the user must confirm here that both codes match.
 */
export function pairWithManualCode(relayUrl: string, manualCode: string, label: string, onSas: (sas: string) => void): PairTask {
  let shownSas: string | null = null;
  return pairOverRelay(
    relayUrl,
    {
      notFound: "Invalid pairing code — check RoALT Settings → Remote.",
      rateLimited: "Too many pairing attempts on the relay right now. Try again in a minute, or scan the QR code.",
    },
    async () => {
      const device = await newDeviceIdentity();
      const ephem = await generateEphemeralEcdh();
      const nonce = crypto.getRandomValues(new Uint8Array(32));
      const commit = await pairCommit(ephem.publicKeyRaw, nonce);
      let session: { pairId: string; pairKey: CryptoKey; transcript: Uint8Array } | null = null;
      return {
        first: { pairId: manualCode.trim(), msg: { mode: "manual_commit", commitB64: toB64Url(commit) } },
        step: async (outer, join) => {
          if (outer.mode === "manual_challenge" && !session) {
            const pairId = typeof outer.pairId === "string" ? outer.pairId : "";
            const pcEphem = typeof outer.pcEphemPubB64 === "string" ? fromB64Url(outer.pcEphemPubB64) : EMPTY;
            if (!PAIR_ID_RE.test(pairId) || pcEphem.length !== 65) throw new Error("Malformed challenge");
            const transcript = await pairTranscript(PAIR_MODE_MANUAL, pairId, pcEphem, ephem.publicKeyRaw, nonce);
            const { pairKey, sas } = await deriveManualPairKeys(ephem.keyPair.privateKey, pcEphem, transcript);
            const blob = await sealDeviceBinding(pairKey, pairId, transcript, device, label);
            session = { pairId, pairKey, transcript };
            join(pairId, { mode: "manual_reveal", ephemPubB64: toB64Url(ephem.publicKeyRaw), nonceB64: toB64Url(nonce), blob });
            shownSas = sas;
            onSas(sas);
            return null;
          }
          if (outer.mode === "ack" && session && typeof outer.blob === "string") {
            const { pairId, pairKey, transcript } = session;
            const pc = await readPairAck(pairKey, pairId, outer.blob, transcript, device, relayUrl);
            return { pc, pairId, pairKey, done: () => doneMessage(pairKey, pairId, transcript, pc) };
          }
          return null;
        },
      };
    },
    () => shownSas,
  );
}
