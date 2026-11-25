/**
 * End-to-end cryptography matching `src-tauri/src/remote/crypto.rs` byte-for-byte.
 * Uses native WebCrypto (`SubtleCrypto`) P-256 ECDH + ECDSA SHA-256 + HKDF-SHA256 + AES-256-GCM.
 */

export const FRAME_HANDSHAKE = 0x01;
export const FRAME_DATA = 0x02;
export const DIR_PC_TO_VIEWER = 0x01;
export const DIR_VIEWER_TO_PC = 0x02;

export type Bytes = Uint8Array<ArrayBuffer>;

function asBuf(u: Uint8Array): BufferSource {
  return u as unknown as BufferSource;
}

const enc = new TextEncoder();
const HANDSHAKE_DOMAIN = enc.encode("roalt-remote-v1-hs");
const SESSION_INFO = enc.encode("roalt-remote-v1-session");
const QR_PAIR_INFO = enc.encode("roalt-remote-v2-pair-qr");
const MANUAL_PAIR_INFO = enc.encode("roalt-remote-v2-manual");
const MANUAL_SAS_INFO = enc.encode("roalt-remote-v2-sas");
const PAIR_TRANSCRIPT_DOMAIN = enc.encode("roalt-remote-v2-pair");
const PAIR_COMMIT_DOMAIN = enc.encode("roalt-remote-v2-commit");
const PAIR_BLOB_DOMAIN = enc.encode("roalt-remote-v2-blob");
const DEVICE_BIND_DOMAIN = enc.encode("roalt-remote-v2-device");
const PC_BIND_DOMAIN = enc.encode("roalt-remote-v2-pc");

export const PAIR_MODE_QR = 0x01;
export const PAIR_MODE_MANUAL = 0x02;

export function toB64Url(bytes: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function fromB64Url(str: string): Uint8Array {
  const padded = str.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - (str.length % 4)) % 4);
  const bin = atob(padded);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function uuidToBytes(uuid: string): Uint8Array {
  const hex = uuid.replace(/-/g, "");
  if (hex.length !== 32) throw new Error("Invalid UUID");
  const out = new Uint8Array(16);
  for (let i = 0; i < 16; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export function bytesToUuid(bytes: Uint8Array): string {
  if (bytes.length !== 16) throw new Error("Invalid UUID bytes");
  const h = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

export function concat(...parts: Uint8Array[]): Uint8Array {
  const total = parts.reduce((sum, p) => sum + p.length, 0);
  const out = new Uint8Array(total);
  let off = 0;
  for (const p of parts) {
    out.set(p, off);
    off += p.length;
  }
  return out;
}

/** Generates a persistent non-extractable ECDSA P-256 identity keypair (safe to store in IndexedDB). */
export async function generateIdentityKeyPair(): Promise<{ keyPair: CryptoKeyPair; publicKeyRaw: Uint8Array }> {
  const keyPair = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, false, ["sign", "verify"]);
  const raw = new Uint8Array(await crypto.subtle.exportKey("raw", keyPair.publicKey));
  return { keyPair, publicKeyRaw: raw };
}

/** Generates an ephemeral P-256 ECDH keypair for one session or manual pairing. */
export async function generateEphemeralEcdh(): Promise<{ keyPair: CryptoKeyPair; publicKeyRaw: Uint8Array }> {
  const keyPair = await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, false, ["deriveBits"]);
  const raw = new Uint8Array(await crypto.subtle.exportKey("raw", keyPair.publicKey));
  return { keyPair, publicKeyRaw: raw };
}

function handshakeTranscript(role: number, pcId: string, deviceId: string, ephemPub: Uint8Array): Uint8Array {
  return concat(HANDSHAKE_DOMAIN, new Uint8Array([role]), uuidToBytes(pcId), uuidToBytes(deviceId), ephemPub);
}

/** Builds a 130-byte handshake frame: `[0x01][ephem_pub: 65B][ecdsa_sig: 64B]`. */
export async function buildHandshake(
  staticSignKey: CryptoKey,
  ephemPub: Uint8Array,
  role: number,
  pcId: string,
  deviceId: string,
): Promise<Uint8Array> {
  const transcript = handshakeTranscript(role, pcId, deviceId, ephemPub);
  const sig = new Uint8Array(await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, staticSignKey, asBuf(transcript)));
  return concat(new Uint8Array([FRAME_HANDSHAKE]), ephemPub, sig);
}

/** Verifies a 130-byte handshake frame from the peer and returns its 65-byte ephemeral public key. */
export async function verifyHandshake(
  peerStaticPubRaw: Uint8Array,
  frame: Uint8Array,
  expectedRole: number,
  pcId: string,
  deviceId: string,
): Promise<Uint8Array> {
  if (frame.length !== 130 || frame[0] !== FRAME_HANDSHAKE) throw new Error("Bad handshake length");
  const ephemPub = frame.slice(1, 66);
  const sig = frame.slice(66, 130);
  const verifyKey = await crypto.subtle.importKey("raw", asBuf(peerStaticPubRaw), { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
  const transcript = handshakeTranscript(expectedRole, pcId, deviceId, ephemPub);
  const ok = await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, verifyKey, asBuf(sig), asBuf(transcript));
  if (!ok) throw new Error("Handshake signature invalid");
  return ephemPub;
}

async function hkdfExpandBytes(ikm: Uint8Array, salt: Uint8Array, info: Uint8Array, lengthBytes: number): Promise<Uint8Array> {
  const baseKey = await crypto.subtle.importKey("raw", asBuf(ikm), "HKDF", false, ["deriveBits"]);
  const bits = await crypto.subtle.deriveBits({ name: "HKDF", hash: "SHA-256", salt: asBuf(salt), info: asBuf(info) }, baseKey, lengthBytes * 8);
  return new Uint8Array(bits);
}

async function importAesKey(raw: Uint8Array): Promise<CryptoKey> {
  return crypto.subtle.importKey("raw", asBuf(raw), { name: "AES-GCM" }, false, ["encrypt", "decrypt"]);
}

function nonceAndAad(dir: number, seq: bigint): { nonce: Uint8Array; aad: Uint8Array } {
  const nonce = new Uint8Array(12);
  nonce[0] = dir;
  const viewN = new DataView(nonce.buffer);
  viewN.setBigUint64(4, seq, false);

  const aad = new Uint8Array(10);
  aad[0] = 0x01;
  aad[1] = dir;
  const viewA = new DataView(aad.buffer);
  viewA.setBigUint64(2, seq, false);
  return { nonce, aad };
}

/** Bidirectional AES-256-GCM session cipher with replay protection. */
export class SessionCipher {
  private txSeq = 1n;
  private rxSeqNext = 1n;

  private constructor(
    private readonly txKey: CryptoKey,
    private readonly rxKey: CryptoKey,
    private readonly txDir: number,
    private readonly rxDir: number,
  ) {}

  static async derive(
    myEphemPriv: CryptoKey,
    peerEphemPubRaw: Uint8Array,
    pcEphemPubRaw: Uint8Array,
    viewerEphemPubRaw: Uint8Array,
    isPc = false,
  ): Promise<SessionCipher> {
    const peerKey = await crypto.subtle.importKey("raw", asBuf(peerEphemPubRaw), { name: "ECDH", namedCurve: "P-256" }, false, []);
    const shared = new Uint8Array(await crypto.subtle.deriveBits({ name: "ECDH", public: peerKey }, myEphemPriv, 256));
    const salt = concat(pcEphemPubRaw, viewerEphemPubRaw);
    const okm = await hkdfExpandBytes(shared, salt, SESSION_INFO, 64);
    const kPc = await importAesKey(okm.slice(0, 32));
    const kViewer = await importAesKey(okm.slice(32, 64));
    return isPc
      ? new SessionCipher(kPc, kViewer, DIR_PC_TO_VIEWER, DIR_VIEWER_TO_PC)
      : new SessionCipher(kViewer, kPc, DIR_VIEWER_TO_PC, DIR_PC_TO_VIEWER);
  }

  async seal(plaintext: Uint8Array): Promise<Uint8Array> {
    const seq = this.txSeq;
    this.txSeq += 1n;
    const { nonce, aad } = nonceAndAad(this.txDir, seq);
    const ct = new Uint8Array(
      await crypto.subtle.encrypt({ name: "AES-GCM", iv: asBuf(nonce), additionalData: asBuf(aad) }, this.txKey, asBuf(plaintext)),
    );
    const header = new Uint8Array(9);
    header[0] = FRAME_DATA;
    new DataView(header.buffer).setBigUint64(1, seq, false);
    return concat(header, ct);
  }

  async open(frame: Uint8Array): Promise<Uint8Array> {
    if (frame.length < 9 + 16 || frame[0] !== FRAME_DATA) throw new Error("Bad data frame");
    const seq = new DataView(frame.buffer, frame.byteOffset, frame.byteLength).getBigUint64(1, false);
    if (seq < this.rxSeqNext) throw new Error("Replay detected");
    const { nonce, aad } = nonceAndAad(this.rxDir, seq);
    const pt = new Uint8Array(
      await crypto.subtle.decrypt(
        { name: "AES-GCM", iv: asBuf(nonce), additionalData: asBuf(aad) },
        this.rxKey,
        asBuf(frame.slice(9)),
      ),
    );
    this.rxSeqNext = seq + 1n;
    return pt;
  }
}

export async function deriveQrPairKey(pairSecret: Uint8Array, pairId: string): Promise<CryptoKey> {
  const raw = await hkdfExpandBytes(pairSecret, enc.encode(pairId), QR_PAIR_INFO, 32);
  return importAesKey(raw);
}

async function sha256(data: Uint8Array): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", asBuf(data)));
}

/** Pairing transcript hash; must match `pair_transcript` in crypto.rs (QR passes empty ephemerals). */
export async function pairTranscript(
  mode: number,
  pairId: string,
  pcEphemPub: Uint8Array,
  browserEphemPub: Uint8Array,
  nonce: Uint8Array,
): Promise<Uint8Array> {
  const id = enc.encode(pairId);
  if (id.length > 255 || nonce.length !== 32) throw new Error("Bad pairing transcript input");
  return sha256(concat(PAIR_TRANSCRIPT_DOMAIN, new Uint8Array([mode, id.length]), id, pcEphemPub, browserEphemPub, nonce));
}

/** Commitment to the browser's ephemeral key, sent before it sees the PC's ephemeral key. */
export async function pairCommit(browserEphemPub: Uint8Array, nonce: Uint8Array): Promise<Uint8Array> {
  return sha256(concat(PAIR_COMMIT_DOMAIN, browserEphemPub, nonce));
}

export async function deriveManualPairKeys(
  myEphemPriv: CryptoKey,
  peerEphemPubRaw: Uint8Array,
  transcript: Uint8Array,
): Promise<{ pairKey: CryptoKey; sas: string }> {
  const peerKey = await crypto.subtle.importKey("raw", asBuf(peerEphemPubRaw), { name: "ECDH", namedCurve: "P-256" }, false, []);
  const shared = new Uint8Array(await crypto.subtle.deriveBits({ name: "ECDH", public: peerKey }, myEphemPriv, 256));
  const rawKey = await hkdfExpandBytes(shared, transcript, MANUAL_PAIR_INFO, 32);
  const sasBytes = await hkdfExpandBytes(shared, transcript, MANUAL_SAS_INFO, 4);
  const num = new DataView(sasBytes.buffer, sasBytes.byteOffset, 4).getUint32(0, false) % 1_000_000;
  const sas = `${String(Math.floor(num / 1000)).padStart(3, "0")} ${String(num % 1000).padStart(3, "0")}`;
  return { pairKey: await importAesKey(rawKey), sas };
}

function pairBlobAad(dir: number, pairId: string): Uint8Array {
  return concat(PAIR_BLOB_DOMAIN, new Uint8Array([dir]), enc.encode(pairId));
}

/** Seals a pairing payload for one direction (`DIR_VIEWER_TO_PC` or `DIR_PC_TO_VIEWER`). */
export async function sealPairBlob(key: CryptoKey, dir: number, pairId: string, plaintext: Uint8Array): Promise<string> {
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const ct = new Uint8Array(
    await crypto.subtle.encrypt({ name: "AES-GCM", iv: asBuf(nonce), additionalData: asBuf(pairBlobAad(dir, pairId)) }, key, asBuf(plaintext)),
  );
  return toB64Url(concat(nonce, ct));
}

export async function openPairBlob(key: CryptoKey, dir: number, pairId: string, b64: string): Promise<Uint8Array> {
  const blob = fromB64Url(b64);
  if (blob.length < 12 + 16) throw new Error("Bad pairing blob");
  const nonce = blob.slice(0, 12);
  const ct = blob.slice(12);
  return new Uint8Array(
    await crypto.subtle.decrypt({ name: "AES-GCM", iv: asBuf(nonce), additionalData: asBuf(pairBlobAad(dir, pairId)) }, key, asBuf(ct)),
  );
}

/** Proves this browser holds `devicePub`'s private key for this pairing transcript (64-byte P1363). */
export async function signDeviceBinding(
  deviceSignKey: CryptoKey,
  transcript: Uint8Array,
  deviceId: string,
  devicePub: Uint8Array,
): Promise<Uint8Array> {
  const msg = concat(DEVICE_BIND_DOMAIN, transcript, uuidToBytes(deviceId), devicePub);
  return new Uint8Array(await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, deviceSignKey, asBuf(msg)));
}

/** Checks the PC's signature binding its static key `pcPub` to this pairing transcript. */
export async function verifyPcBinding(
  pcPub: Uint8Array,
  transcript: Uint8Array,
  pcId: string,
  deviceId: string,
  sig: Uint8Array,
): Promise<boolean> {
  if (pcPub.length !== 65 || pcPub[0] !== 0x04 || sig.length !== 64) return false;
  const key = await crypto.subtle.importKey("raw", asBuf(pcPub), { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
  const msg = concat(PC_BIND_DOMAIN, transcript, uuidToBytes(pcId), pcPub, uuidToBytes(deviceId));
  return crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, key, asBuf(sig), asBuf(msg));
}
