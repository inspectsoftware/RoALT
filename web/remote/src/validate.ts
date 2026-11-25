import type { ParsedInput, RemoteEvent } from "./types";

type Check = (v: unknown) => boolean;

const MAX_ITEMS = 10_000;

const str: Check = (v) => typeof v === "string";
const num: Check = (v) => typeof v === "number" && Number.isFinite(v);
const bool: Check = (v) => typeof v === "boolean";
const opt = (c: Check): Check => (v) => v == null || c(v);
const arr = (c: Check): Check => (v) => Array.isArray(v) && v.length <= MAX_ITEMS && v.every(c);
const oneOf = (...xs: string[]): Check => (v) => typeof v === "string" && xs.includes(v);
const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const obj = (shape: Record<string, Check>): Check => (v) => isObj(v) && Object.entries(shape).every(([k, c]) => c(v[k]));
const byKind = (field: string, shapes: Record<string, Check>): Check => (v) =>
  isObj(v) && typeof v[field] === "string" && Object.hasOwn(shapes, v[field]) && shapes[v[field]](v);

const account = obj({
  id: num,
  username: str,
  displayName: str,
  alias: str,
  color: opt(str),
  main: bool,
  status: oneOf("unknown", "valid", "invalid", "challenged"),
  groupIds: arr(num),
});
const group = obj({ id: num, name: str });
const favourite = obj({ id: num, name: str, placeId: opt(num) });
const limits = obj({ cpuCap: opt(num), eco: bool, trimEveryMin: opt(num), affinity: opt(num) });
const instance = obj({
  id: str,
  accountId: num,
  state: oneOf("starting", "joining", "inGame", "disconnected", "exited"),
  target: obj({ kind: str, placeId: opt(num) }),
  placeId: opt(num),
  startedAt: num,
  endedAt: opt(num),
  note: opt(str),
  limits,
  antiAfk: bool,
  fpsCap: opt(num),
  relaunching: bool,
});
const metrics = obj({
  instanceId: str,
  pid: num,
  cpuPct: opt(num),
  memoryMb: num,
  privateMb: num,
  readBps: opt(num),
  writeBps: opt(num),
  gpuPct: opt(num),
  gpuMemMb: opt(num),
});
const systemUsage = obj({ cpuPct: opt(num), memUsedMb: num, memTotalMb: num });
const empty = obj({});
const bulkStage = byKind("kind", {
  queued: empty,
  ticket: empty,
  starting: empty,
  joining: empty,
  inGame: empty,
  done: empty,
  failed: obj({ message: str }),
  cancelled: empty,
});
const bulkItem = obj({ accountId: num, userId: num, stage: bulkStage, instanceId: opt(str) });
const bulk = obj({ batchId: str, done: bool, items: arr(bulkItem) });
const crash = obj({ id: num, accountId: num, kind: oneOf("crash", "disconnect", "hang", "mem", "timed"), at: num, reason: opt(str) });
const snapshot = obj({
  pcName: str,
  appVersion: str,
  role: oneOf("control", "readonly"),
  vaultLocked: bool,
  windowsLocked: bool,
  accounts: arr(account),
  groups: arr(group),
  favourites: arr(favourite),
  instances: arr(instance),
  metrics: arr(metrics),
  systemUsage: opt(systemUsage),
  bulkProgress: opt(bulk),
  shutdownAt: opt(num),
  crashes: arr(crash),
});
const page = byKind("list", {
  groups: obj({ items: arr(group) }),
  favourites: obj({ items: arr(favourite) }),
  instances: obj({ items: arr(instance) }),
  metrics: obj({ items: arr(metrics) }),
  crashes: obj({ items: arr(crash) }),
  bulkItems: obj({ items: arr(bulkItem) }),
});

const event = byKind("t", {
  snapshot: obj({ batch: num, data: snapshot }),
  accountsUpdated: obj({ batch: num, accounts: arr(account), groups: arr(group) }),
  accountsPage: obj({ batch: num, offset: num, accounts: arr(account) }),
  page: obj({ batch: num, offset: num, page }),
  instancesUpdated: obj({ batch: num, instances: arr(instance) }),
  metricsUpdated: obj({ batch: num, metrics: arr(metrics), systemUsage: opt(systemUsage), windowsLocked: bool, vaultLocked: bool, shutdownAt: opt(num) }),
  bulkProgress: obj({ batch: num, progress: bulk }),
  rpcResult: obj({ reqId: num, ok: bool, error: opt(str), code: opt(str) }),
  notice: obj({ error: bool, message: str }),
  webRtcSignal: obj({ instanceId: str }),
});

const launchTarget = byKind("kind", {
  place: obj({ placeId: num, launchData: opt(str) }),
  server: obj({ placeId: num, jobId: str }),
  privateLink: obj({ placeId: num, linkCode: str }),
  privateAccess: obj({ placeId: num, accessCode: str }),
  shareLink: obj({ code: str, linkType: str }),
  shareLinkV2: obj({ code: str }),
  universe: obj({ universeId: num }),
  followUser: obj({ userId: num }),
});

const parsedInput = byKind("kind", {
  target: obj({ target: launchTarget }),
  jobIdOnly: obj({ jobId: str }),
  username: obj({ name: str }),
  invalid: obj({ reason: str }),
  unsupported: obj({ reason: str }),
});

const HIDDEN = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\u034F\u115F\u1160\u17B4\u17B5\u180B-\u180F\u2800\u3164\uFE00-\uFE0F\uFFA0]/gu;

/** A PC name as shown: no control, format (bidi, zero-width) or blank-drawing characters, no
 * stacked marks, spaces collapsed. */
export function cleanName(name: string): string {
  return name.replace(HIDDEN, "").replace(/(\p{M})\p{M}+/gu, "$1").replace(/\s+/gu, " ").trim();
}

/** Look-alike letters from other scripts, mapped to the Latin letter they pass for. */
const LOOKALIKE: Record<string, string> = {
  а: "a", в: "b", е: "e", ё: "e", к: "k", м: "m", н: "h", о: "o", р: "p", с: "c", т: "t", у: "y", х: "x",
  і: "i", ї: "i", ј: "j", ѕ: "s", ԁ: "d", ӏ: "l", ɡ: "g",
  α: "a", β: "b", ε: "e", η: "n", ι: "i", κ: "k", ν: "v", ο: "o", ρ: "p", τ: "t", υ: "u", χ: "x",
  օ: "o", ս: "u", հ: "h", ո: "n", ց: "g", ք: "p", զ: "q",
};
const CHEROKEE: Record<string, string> = {
  Ꭺ: "a", Ᏼ: "b", Ꮯ: "c", Ꭰ: "d", Ꭼ: "e", Ꮐ: "g", Ꮋ: "h", Ꭻ: "j", Ꮶ: "k", Ꮮ: "l", Ꮇ: "m", Ꮲ: "p", Ꭱ: "r", Ꮪ: "s", Ꭲ: "t", Ꮩ: "v", Ꮃ: "w", Ꮓ: "z",
};
for (const [c, latin] of Object.entries(CHEROKEE)) LOOKALIKE[c.toLowerCase()] = latin;

/** The key two PC names clash on: cleaned, NFKC-folded, lowercased, look-alikes mapped. */
export function nameKey(name: string): string {
  return [...cleanName(name).normalize("NFKC").toLowerCase()].map((c) => LOOKALIKE[c] ?? c).join("").replace(/\s+/gu, "");
}

/** A decrypted PC event with every field the dashboard reads checked, or null. */
export function parseEvent(v: unknown): RemoteEvent | null {
  return event(v) ? (v as RemoteEvent) : null;
}

/** The `data` of a `targetParse` result, or null when it is not a well-formed `{ parsed }`. */
export function parseTargetParse(data: unknown): ParsedInput | null {
  return isObj(data) && parsedInput(data.parsed) ? (data.parsed as ParsedInput) : null;
}
