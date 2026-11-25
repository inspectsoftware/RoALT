import { generateIdentityKeyPair } from "./crypto";
import type { RemoteRole } from "./types";

const DB_NAME = "roalt-remote-v1";
const STORE = "kv";
const PCS_KEY = "pcs";
/** A re-pairing with no session yet swaps with the other device this often, until one of them has one. */
export const UNCONFIRMED_FALLBACK_MS = 15 * 60_000;

/** The device credentials this browser holds for one PC. */
export type PairedDevice = {
  deviceId: string;
  keyPair: CryptoKeyPair;
  roomToken: string;
  role: RemoteRole;
};

/** One paired PC, with the deviceId and non-extractable key this browser uses for that PC only.
 * `previous` is the other device of a re-pairing, kept until one of the two had a session;
 * `addedAt` is when the current device became current. */
export type PairedPc = {
  pcId: string;
  pcName: string;
  pcPubB64: string;
  roomToken: string;
  relayUrl: string;
  role: RemoteRole;
  addedAt: number;
  deviceId: string;
  keyPair: CryptoKeyPair;
  previous?: PairedDevice;
};

export type DeviceIdentity = {
  deviceId: string;
  keyPair: CryptoKeyPair;
  publicKeyRaw: Uint8Array;
};

let opened: Promise<IDBDatabase> | null = null;

function openDb(): Promise<IDBDatabase> {
  opened ??= new Promise<IDBDatabase>((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, 1);
    req.onupgradeneeded = () => {
      if (!req.result.objectStoreNames.contains(STORE)) {
        req.result.createObjectStore(STORE);
      }
    };
    req.onsuccess = () => {
      const db = req.result;
      db.onversionchange = () => {
        db.close();
        opened = null;
      };
      db.onclose = () => (opened = null);
      resolve(db);
    };
    req.onerror = () => reject(req.error);
    req.onblocked = () => reject(new Error("Encrypted storage is held by another tab of this dashboard. Close the other tabs and try again."));
  }).catch((err: unknown) => {
    opened = null;
    throw err;
  });
  return opened;
}

async function changePcs(change: (list: PairedPc[]) => PairedPc[] | null): Promise<PairedPc[] | null> {
  const db = await openDb();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE, "readwrite");
    const store = tx.objectStore(STORE);
    let next: PairedPc[] | null = null;
    const req = store.get(PCS_KEY);
    req.onsuccess = () => {
      const stored = readPcs(req.result);
      try {
        next = change(stored);
      } catch (err) {
        tx.abort();
        reject(err);
        return;
      }
      if (next && next !== stored) store.put(next, PCS_KEY);
    };
    tx.oncomplete = () => resolve(next);
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error ?? new Error("Storage change aborted"));
  });
}

/** A fresh deviceId + non-extractable P-256 key for pairing with one PC, so no two PCs (nor the relay) can link them. */
export async function newDeviceIdentity(): Promise<DeviceIdentity> {
  const { keyPair, publicKeyRaw } = await generateIdentityKeyPair();
  return { deviceId: crypto.randomUUID(), keyPair, publicKeyRaw };
}

function isDevice(v: unknown): v is PairedDevice {
  const d = v as Partial<PairedDevice> | null;
  return !!d && typeof d.deviceId === "string" && typeof d.roomToken === "string" && d.keyPair?.privateKey instanceof CryptoKey;
}

function isPairedPc(v: unknown): v is PairedPc {
  const p = v as Partial<PairedPc> | null;
  return (
    isDevice(v) &&
    !!p &&
    typeof p.pcId === "string" &&
    typeof p.pcName === "string" &&
    typeof p.pcPubB64 === "string" &&
    typeof p.relayUrl === "string"
  );
}

const roleOf = (role: unknown): RemoteRole => (role === "control" ? "control" : "readonly");

function readPcs(stored: unknown): PairedPc[] {
  return (Array.isArray(stored) ? stored : []).filter(isPairedPc).map((p): PairedPc => {
    const previous = isDevice(p.previous) ? { ...p.previous, role: roleOf(p.previous.role) } : undefined;
    return { ...p, role: roleOf(p.role), previous };
  });
}

export async function loadPairedPcs(): Promise<PairedPc[]> {
  const db = await openDb();
  return new Promise((resolve, reject) => {
    const req = db.transaction(STORE, "readonly").objectStore(STORE).get(PCS_KEY);
    req.onsuccess = () => resolve(readPcs(req.result));
    req.onerror = () => reject(req.error);
  });
}

const deviceOf = (p: PairedPc): PairedDevice => ({ deviceId: p.deviceId, keyPair: p.keyPair, roomToken: p.roomToken, role: p.role });

/** Every device this browser holds for `pc`'s PC besides `pc`'s own; a re-pairing proves each one to the PC. */
export function devicesReplacedBy(list: PairedPc[], pc: Pick<PairedPc, "pcId" | "pcPubB64" | "deviceId">): PairedDevice[] {
  const old = list.find((p) => p.pcId === pc.pcId && p.pcPubB64 === pc.pcPubB64 && p.deviceId !== pc.deviceId);
  return old ? [deviceOf(old), ...(old.previous ? [old.previous] : [])].filter((d) => d.deviceId !== pc.deviceId) : [];
}

/** Why `pc` can't be stored next to `list`: a stored PC with the same pcId has a different key, or
 * the last re-pairing of it hasn't had a session yet, so the PC may still hold either of two devices. */
export function whyNotStored(list: PairedPc[], pc: Pick<PairedPc, "pcId" | "pcPubB64">): string | null {
  const same = list.find((p) => p.pcId === pc.pcId);
  if (same && same.pcPubB64 !== pc.pcPubB64) {
    return `This PC claims to be "${same.pcName}", which is already paired with a different key. Forget "${same.pcName}" first if you replaced it.`;
  }
  if (same?.previous) {
    return `The last pairing with "${same.pcName}" hasn't connected yet. Open "${same.pcName}" on this dashboard while RoALT runs on it until it connects, or forget it here first, then pair again.`;
  }
  return null;
}

/** Saves a newly paired PC; refuses what `whyNotStored` refuses. A re-pairing keeps the device it
 * replaces as `previous` until one of the two had a session. */
export async function upsertPairedPc(pc: PairedPc): Promise<PairedPc[]> {
  const next = await changePcs((list) => {
    const refusal = whyNotStored(list, pc);
    if (refusal) throw new Error(refusal);
    const old = list.find((p) => p.pcId === pc.pcId && p.deviceId !== pc.deviceId);
    return [{ ...pc, previous: old && deviceOf(old) }, ...list.filter((p) => p.pcId !== pc.pcId)];
  });
  return next ?? [];
}

/** `deviceId`, either device of a re-pairing of `pcId`, had a session: it stays and the other goes.
 * Returns the stored PCs, changed or not, so a tab whose copy is out of date takes them over. */
export async function confirmPairedPc(pcId: string, deviceId: string): Promise<PairedPc[]> {
  const next = await changePcs((list) => {
    const pc = list.find((p) => p.pcId === pcId && (p.deviceId === deviceId || p.previous?.deviceId === deviceId));
    if (!pc?.previous) return list;
    const kept = pc.deviceId === deviceId ? pc : { ...pc, ...pc.previous };
    return list.map((p) => (p === pc ? { ...kept, previous: undefined } : p));
  });
  return next ?? [];
}

/** Re-pairings with no session `UNCONFIRMED_FALLBACK_MS` after their device became current swap to
 * the other device and keep both, so a device the PC may still know is never thrown away (pure). */
export function fallBackStale(list: PairedPc[], now: number): PairedPc[] | null {
  let changed = false;
  const next = list.map((p) => {
    if (!p.previous || now - p.addedAt < UNCONFIRMED_FALLBACK_MS) return p;
    changed = true;
    return { ...p, ...p.previous, addedAt: now, previous: deviceOf(p) };
  });
  return changed ? next : null;
}

export function fallBackStalePairings(now: number): Promise<PairedPc[] | null> {
  return changePcs((list) => fallBackStale(list, now));
}

export async function removePairedPc(pcId: string): Promise<PairedPc[]> {
  return (await changePcs((list) => list.filter((p) => p.pcId !== pcId))) ?? [];
}

export function isLocalDashboard(): boolean {
  return typeof window !== "undefined" && (window.location.hostname === "localhost" || window.location.hostname === "127.0.0.1");
}

/** The hosted dashboard only ever talks to its own relay; `?relay=` works only on a local dev build. */
export function defaultRelayUrl(): string {
  if (isLocalDashboard()) {
    const param = new URLSearchParams(window.location.search).get("relay");
    if (param && (param.startsWith("wss://") || param.startsWith("ws://localhost") || param.startsWith("ws://127.0.0.1"))) return param;
    return "ws://127.0.0.1:8788/ws/remote";
  }
  return "wss://remote.roalt.dev/ws/remote";
}

export function detectDeviceLabel(): string {
  const ua = navigator.userAgent || "";
  const os = /iPhone/.test(ua)
    ? "iPhone"
    : /iPad/.test(ua)
      ? "iPad"
      : /Android/.test(ua)
        ? "Android"
        : /Mac OS X/.test(ua)
          ? "Mac"
          : /Windows/.test(ua)
            ? "Windows"
            : "Browser";
  const browser = /Edg\//.test(ua)
    ? "Edge"
    : /Firefox\//.test(ua)
      ? "Firefox"
      : /Chrome\//.test(ua)
        ? "Chrome"
        : /Safari\//.test(ua)
          ? "Safari"
          : "Web";
  return `${os} · ${browser}`;
}
