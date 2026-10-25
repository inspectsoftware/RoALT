// Dev-only fake backend so the UI can be previewed in a normal browser (`npm run dev`).
// main.tsx loads it only when import.meta.env.DEV and there is no Tauri runtime.
// Master password for the fake vault: "password". A pasted cookie containing "bad" is rejected.
// Launching as "AFK 1" fails with an expired cookie; "Builder" launches through a bootstrapper.
// A username containing "nobody" doesn't exist.
// Launched clients start, join and play on timers; one fake Roblox client is already running,
// so turning multi-instance on first asks to close it.
import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type {
  AccountInfo, AccountStatus, AccountSummary, Favourite, Group, Instance, JoinCandidate, LaunchTarget, MultiInstanceStatus, ParsedInput,
  Presence, Settings, VaultMode,
} from "../api";

const MIN = 60_000;
const DAY = 86_400_000;
const now = Date.now();

const groups: Group[] = [
  { id: 1, name: "main", color: "#C6F432", sortOrder: 0 },
  { id: 2, name: "farm", color: "#7AB0FF", sortOrder: 1 },
  { id: 3, name: "afk", color: "#5FE0C8", sortOrder: 2 },
  { id: 4, name: "trade", color: "#FFA05C", sortOrder: 3 },
  { id: 5, name: "dev", color: "#B49CFF", sortOrder: 4 },
];

const seed: [string, string, string, number, string | null, AccountStatus, number[], boolean][] = [
  ["Main", "Nova", "nova_builds", 2840197733, "#C6F432", "valid", [1], true],
  ["Alt 01", "Nova Alt", "nova_alt01", 4519920471, "#7AB0FF", "valid", [2], false],
  ["Alt 02", "Nova Alt", "nova_alt02", 4519987310, "#7AB0FF", "valid", [2], false],
  ["Trader", "Quiet Market", "quietmarket", 1977304452, "#FFA05C", "challenged", [4], true],
  ["AFK 1", "unit one", "afk_unit_1", 5102947781, "#5FE0C8", "valid", [3], false],
  ["AFK 2", "unit two", "afk_unit_2", 5102951106, "#5FE0C8", "invalid", [3], false],
  ["Builder", "Nova Studio", "studio_nova", 3308812290, "#B49CFF", "valid", [5], false],
  ["", "test acc", "nova_test_acc", 5560021849, null, "valid", [5], false],
  ["Event", "Nova Events", "nova_event", 4870033617, "#FF8FC7", "unknown", [], false],
];

let accounts: AccountSummary[] = seed.map(([alias, displayName, username, userId, color, status, groupIds, favourite], i) => ({
  id: i + 1,
  userId,
  username,
  displayName,
  alias,
  notes: i === 0 ? "Main account. Don't trade from here." : "",
  color,
  favourite,
  sortOrder: i,
  groupIds,
  status,
  statusCheckedAt: status === "unknown" ? null : now - (i + 2) * MIN,
  createdAt: now - (40 - i * 3) * DAY,
}));

let settings: Settings = {
  theme: "dark", density: "detailed", autoLockMinutes: 15, clipboardClearSeconds: 30, cacheMinutes: 10,
  multiInstance: false, protectSharedLogin: true,
};
// Instances: a stranger client blocks multi-instance until "Close Roblox".
let strangerRunning = true;
let lockHeld = false;
let instances: Instance[] = [];
const instancesChanged = () => void emit("instances://changed");
const multiStatus = (): MultiInstanceStatus => {
  const blockedBy = lockHeld ? [] : [...(strangerRunning ? [9120] : []), ...instances.filter((i) => i.state !== "exited").map((i) => i.pid)];
  const message = !settings.multiInstance || lockHeld ? null
    : blockedBy.length ? "Roblox is already running. Close it (including its tray icon) to run several clients."
    : "Multi-instance isn't active yet. Turn it off and on again to retry.";
  return { enabled: settings.multiInstance, active: lockHeld, protectSharedLogin: settings.protectSharedLogin, blockedBy, message };
};
// A client launched without the lock owns Roblox's singleton event; clients started under the
// lock don't block taking it again.
let eventHolder: string | null = null;
const retakeLock = () => {
  const holder = instances.some((i) => i.id === eventHolder && i.state !== "exited");
  if (settings.multiInstance && !lockHeld && !strangerRunning && !holder) lockHeld = true;
};
function endInstance(id: string, note: string) {
  const i = instances.find((x) => x.id === id);
  if (!i || i.state === "exited") return;
  i.state = "exited";
  i.endedAt = Date.now();
  i.note = note;
  instancesChanged();
}
function track(accountId: number, pid: number, target: LaunchTarget, placeId: number | null, viaBootstrapper: boolean) {
  // Without the lock a new client replaces the running one, as Roblox does.
  if (!lockHeld) for (const old of instances) endInstance(old.id, "Replaced by another Roblox client");
  const i: Instance = {
    id: crypto.randomUUID(), accountId, pid, state: "starting", target, placeId, jobId: null,
    startedAt: Date.now(), endedAt: null, viaBootstrapper, note: null,
  };
  instances.push(i);
  if (!lockHeld) eventHolder = i.id;
  instancesChanged();
  const step = (ms: number, f: () => void) => setTimeout(() => { if (i.state !== "exited") { f(); instancesChanged(); } }, ms);
  step(1500, () => { i.state = "joining"; });
  step(4000, () => { i.state = "inGame"; i.jobId = crypto.randomUUID(); i.placeId ??= 1818; });
}
const vault = { mode: "master" as VaultMode, locked: false, password: "password" };
const FAKE_COOKIE = "_|WARNING:-DO-NOT-SHARE-THIS.--DEV-PREVIEW-FAKE-VALUE|_" + "0F1E2D3C4B5A69788796A5B4C3D2E1F0".repeat(6);

type Args = Record<string, any>;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const fail = (kind: string, message: string) => Promise.reject({ kind, message });
const changed = () => void emit("accounts://changed");
const find = (id: number) => accounts.find((a) => a.id === id);

function addFake(username: string): AccountSummary {
  const id = Math.max(0, ...accounts.map((a) => a.id)) + 1;
  const a: AccountSummary = {
    id, userId: 6000000000 + id, username, displayName: username.replace(/_/g, " "), alias: "", notes: "", color: null,
    favourite: false, sortOrder: accounts.length, groupIds: [], status: "valid", statusCheckedAt: Date.now(), createdAt: Date.now(),
  };
  accounts.push(a);
  changed();
  return a;
}

// ---------- M2: headshots, presence, account info ----------

/** A blocky head on a transparent background, so the account colour shows behind it. */
function headshot(seed: number) {
  const skin = ["#F5CD9B", "#E0A878", "#C68642", "#8D5524", "#FFDBAC"][seed % 5];
  const shirt = ["#2B2F36", "#3A6EA5", "#7A3E9D", "#2E7D5B", "#B23A48"][seed % 7 % 5];
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><rect x="8" y="36" width="32" height="12" fill="${shirt}"/><rect x="13" y="9" width="22" height="24" fill="${skin}"/><rect x="18" y="18" width="3" height="4" fill="#111"/><rect x="27" y="18" width="3" height="4" fill="#111"/><rect x="20" y="27" width="8" height="2" fill="#111"/></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}
const NO_HEADSHOT = 8; // shows the initials fallback

const SKY: Presence = {
  kind: "inGame", lastLocation: "Sky Islands", placeId: 4410028153, rootPlaceId: 4410028153, universeId: 1559201384,
  gameId: "8f3a1c2e-77b0-4c1d-9e52-0b6c1d3fa911",
};
const OBBY: Presence = {
  kind: "inGame", lastLocation: "Mega Obby", placeId: 7719004215, rootPlaceId: 7719004215, universeId: 2650339475,
  gameId: "1c0e9b44-3a2f-4f6e-a1d8-55e0c2b97a03",
};
const plain = (kind: Presence["kind"], lastLocation: string | null = null): Presence =>
  ({ kind, lastLocation, placeId: null, rootPlaceId: null, universeId: null, gameId: null });
const PRESENCE: Record<number, Presence> = { 1: SKY, 2: plain("online", "Website"), 3: SKY, 5: OBBY, 7: plain("studio", "Studio") };
const presenceOf = (a: AccountSummary) => PRESENCE[a.id] ?? plain("offline");

const ABOUT: Record<number, string> = {
  1: [
    "Builder and scripter. Sky Islands team since 2019.",
    "Commissions open for maps, UI and small systems; message me first.",
    "No trades, no group invites, please. Alts: nova_alt01, nova_alt02, nova_event.",
    "If you find a bug in one of my games, the group wall is the fastest way to reach me.",
  ].join("\n"),
  4: "Fair trades only. Check my rap before asking.",
  7: "Studio account for testing places.",
};

const infos = new Map<number, AccountInfo>();

function makeInfo(a: AccountSummary): AccountInfo {
  const i = a.id;
  const r = (n: number) => (a.userId % 9973) * (i + 3) % n;
  // A dead cookie still gets the public parts, like the real backend.
  const missing = a.status === "invalid" ? ["robux", "presence", "email", "2-step"] : [];
  if (i === NO_HEADSHOT) missing.push("avatar");
  const has = (part: string) => !missing.includes(part);
  return {
    accountId: i, userId: a.userId, username: a.username, displayName: a.displayName,
    description: ABOUT[i] ?? "",
    created: new Date(now - (420 + r(2400)) * DAY).toISOString(),
    isBanned: i === 9,
    verifiedBadge: i === 1,
    headshotUrl: has("avatar") ? headshot(a.userId) : null,
    robux: has("robux") ? (i === 1 ? 1_284_500 : r(15000)) : null,
    subscription: i === 1 ? "plus" : i === 4 ? "premium" : "none",
    friends: r(200), followers: r(1500), following: r(90),
    emailVerified: has("email") ? i % 3 !== 2 : null,
    twoStepMethods: has("2-step") ? (i % 4 === 3 ? [] : ["Authenticator", "Email"].slice(0, 1 + (i % 2))) : null,
    presence: has("presence") ? presenceOf(a) : null,
    fetchedAt: Date.now(),
    unavailable: missing,
  };
}

// ---------- M3: target parsing, launch, favourites, join ----------

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const SHARE_TYPES = ["Server", "ExperienceInvite", "ExperienceDetails", "ExperienceAffiliate", "ExperienceEvent", "Profile"];
const idOf = (v?: string) => (v && /^\d{1,15}$/.test(v) && Number(v) > 0 ? Number(v) : null);
const target = (t: LaunchTarget): ParsedInput => ({ kind: "target", target: t });
const bad = (reason: string): ParsedInput => ({ kind: "invalid", reason });
const follow = (v?: string) => (idOf(v) ? target({ kind: "followUser", userId: Number(v) }) : bad("That user ID isn't valid."));

/** A small stand-in for launcher::target::parse (the real rules: docs/research/ref_parse.py). */
function parseTarget(raw: string): ParsedInput {
  const s = raw.trim().replace(/^<|>$/g, "");
  if (!s) return bad("Paste a game link, an ID or @username.");
  if (idOf(s)) return target({ kind: "place", placeId: Number(s), launchData: null });
  if (UUID.test(s)) return { kind: "jobIdOnly", jobId: s.toLowerCase() };
  if (/^@?[A-Za-z0-9_]{3,20}$/.test(s)) return { kind: "username", name: s.replace(/^@/, "") };
  if (/^roblox-(player|studio)/i.test(s)) return bad("That launch link carries a sign-in ticket. Paste the game link instead.");
  let u: URL;
  try {
    u = new URL(/^[a-z][a-z0-9+.-]*:/i.test(s) ? s : `https://${s}`);
  } catch {
    return bad("That doesn't look like a link.");
  }
  // First occurrence of a key wins, as in the real parser.
  const q = new Map([...u.searchParams].reverse().map(([k, v]) => [k.toLowerCase(), v]));
  if (u.protocol === "roblox:") {
    const route = `${u.host}${u.pathname}`.replace(/^\/+|\/+$/g, "").toLowerCase();
    if (route === "" || route === "experiences/start") return fromQuery(q);
    if (route === "navigation/share_links") return share(q);
    if (route === "navigation/profile") return follow(q.get("userid"));
    if (route === "navigation/game_details") {
      const universeId = idOf(q.get("gameid"));
      return universeId ? target({ kind: "universe", universeId }) : bad("That game ID isn't valid.");
    }
    return { kind: "unsupported", reason: "That Roblox app link doesn't open a game." };
  }
  if (!/^(www\.|web\.|m\.)?roblox\.com$/i.test(u.hostname)) return bad("Only roblox.com links work here.");
  const seg = u.pathname.split("/").filter(Boolean);
  if (seg.length > 1 && /^[a-z]{2}(-[a-z0-9]{2,4})?$/i.test(seg[0])) seg.shift();
  const [a = "", b] = seg.map((x) => x.toLowerCase());
  if (a === "games" && b === "start") return fromQuery(q);
  if (a === "games") return idOf(b) ? fromQuery(new Map([...q, ["placeid", b]])) : bad("That place ID isn't valid.");
  if (a === "share" || a === "share-links") return share(q);
  if (a === "users") return follow(b);
  return { kind: "unsupported", reason: "That roblox.com page isn't a game." };
}

function fromQuery(q: Map<string, string>): ParsedInput {
  const placeId = idOf(q.get("placeid") ?? q.get("id"));
  if (!placeId) return q.has("userid") ? follow(q.get("userid")) : bad("That place ID isn't valid.");
  const link = q.get("linkcode") ?? q.get("privateserverlinkcode");
  if (link !== undefined) return /^\d{1,64}$/.test(link) ? target({ kind: "privateLink", placeId, linkCode: link }) : bad("That private server link code isn't valid.");
  const access = q.get("accesscode");
  if (access !== undefined) return UUID.test(access) ? target({ kind: "privateAccess", placeId, accessCode: access.toLowerCase() }) : bad("That access code isn't valid.");
  const job = q.get("gameinstanceid");
  if (job !== undefined) return UUID.test(job) ? target({ kind: "server", placeId, jobId: job.toLowerCase() }) : bad("That job ID isn't valid.");
  return target({ kind: "place", placeId, launchData: q.get("launchdata") || null });
}

function share(q: Map<string, string>): ParsedInput {
  const code = q.get("code") ?? "";
  if (!/^[A-Za-z0-9_-]{1,128}$/.test(code)) return bad("That share link's code isn't valid.");
  if (q.get("v")?.toLowerCase() === "v2") return target({ kind: "shareLinkV2", code });
  if (!q.has("type")) return bad("That share link is missing its type.");
  const type = SHARE_TYPES.find((t) => t.toLowerCase() === q.get("type")?.toLowerCase());
  return type ? target({ kind: "shareLink", code, linkType: type }) : { kind: "unsupported", reason: "That share link isn't for a game." };
}

const FRIENDS: JoinCandidate[] = [
  { userId: 1203994411, username: "skyrunner_k", displayName: "Kai", accountId: null, presence: SKY },
  // In a game, but Roblox hides which server: picking this one follows.
  { userId: 998120347, username: "mossy_bricks", displayName: "Moss", accountId: null, presence: { ...OBBY, placeId: null, gameId: null } },
  { userId: 3001772200, username: "lunatrades", displayName: "Luna", accountId: null, presence: plain("online", "Website") },
];

const favSeed: [string, LaunchTarget, number[]][] = [
  ["Sky Islands", { kind: "place", placeId: 4410028153, launchData: null }, [1, 2, 3]],
  ["Sky Islands · VIP", { kind: "shareLink", code: "7c1f0e9a2b4d4e61", linkType: "Server" }, [1]],
  ["Mega Obby", { kind: "place", placeId: 7719004215, launchData: null }, [5]],
  ["Follow @nova_builds", { kind: "followUser", userId: 2840197733 }, [2]],
];
let favourites: Favourite[] = favSeed.map(([name, t, accountIds], i) => ({
  id: i + 1, name, target: t, accountIds, inTray: i === 0, sortOrder: i, lastUsedAt: null,
}));

const placeOfTarget = (t: LaunchTarget) => ("placeId" in t ? t.placeId : t.kind === "shareLink" ? SKY.placeId : null);

function reauth(password?: string) {
  if (vault.mode === "master" && password !== vault.password) return fail("wrongPassword", "Wrong password.");
  return Promise.resolve();
}

async function handle(cmd: string, args: Args): Promise<unknown> {
  switch (cmd) {
    case "vault_status":
      return { mode: vault.mode, locked: vault.locked, accounts: accounts.length, autoLockMinutes: settings.autoLockMinutes };
    case "vault_unlock":
      await sleep(450);
      if (args.password !== vault.password) return fail("wrongPassword", "Wrong password.");
      vault.locked = false;
      return null;
    case "vault_lock":
      if (vault.mode === "master") vault.locked = true;
      void emit("vault://locked");
      return null;
    case "vault_set_mode": {
      await sleep(400);
      if (vault.mode === "master" && args.currentPassword !== vault.password) return fail("wrongPassword", "Wrong password.");
      if (args.mode === "master") {
        if (!args.newPassword || args.newPassword.length < 8) return fail("invalid", "Use at least 8 characters.");
        vault.password = args.newPassword;
      }
      vault.mode = args.mode;
      return null;
    }
    case "app_activity":
      return null;

    case "accounts_list":
      return structuredClone(accounts);
    case "account_add_cookie":
      await sleep(700);
      if (/bad/i.test(args.cookie)) return fail("cookieInvalid", "This cookie is not valid or has expired. Sign in again.");
      return addFake(`pasted_${accounts.length + 1}`);
    case "account_add_login":
      await sleep(3500);
      return addFake(`fresh_login_${accounts.length + 1}`);
    case "account_update": {
      const a = find(args.id);
      if (!a) return fail("notFound", "Not found.");
      const p = args.patch;
      Object.assign(a, p, p.color === undefined ? {} : { color: p.color || null });
      changed();
      return structuredClone(a);
    }
    case "accounts_reorder":
      (args.ids as number[]).forEach((id, i) => {
        const a = find(id);
        if (a) a.sortOrder = i;
      });
      changed();
      return null;
    case "accounts_remove":
      accounts = accounts.filter((a) => !args.ids.includes(a.id));
      changed();
      return null;
    case "accounts_validate": {
      const ids: number[] = args.ids ?? accounts.map((a) => a.id);
      for (const id of ids) {
        await sleep(250);
        const a = find(id);
        if (a && a.status === "unknown") a.status = "valid";
        if (a) a.statusCheckedAt = Date.now();
      }
      changed();
      return structuredClone(accounts);
    }
    case "account_reveal_cookie":
      await reauth(args.password);
      return FAKE_COOKIE;
    case "account_copy_cookie":
      await reauth(args.password);
      return null;

    case "account_info": {
      const a = find(args.id);
      if (!a) return fail("notFound", "Not found.");
      await sleep(args.refresh ? 900 : 600);
      if (args.refresh || !infos.has(a.id)) infos.set(a.id, makeInfo(a));
      return structuredClone(infos.get(a.id));
    }
    case "accounts_headshots":
      await sleep(300);
      return accounts.filter((a) => a.id !== NO_HEADSHOT).map((a) => ({ accountId: a.id, url: headshot(a.userId) }));
    case "accounts_presence":
      await sleep(250);
      return accounts
        .filter((a) => (args.ids as number[]).includes(a.id) && a.status === "valid")
        .map((a) => ({ accountId: a.id, presence: presenceOf(a) }));
    case "account_open_session":
      await sleep(500);
      return find(args.id) ? null : fail("notFound", "Not found.");

    case "target_parse":
      return parseTarget(String(args.input ?? ""));
    case "user_lookup": {
      await sleep(400);
      const name = String(args.name ?? "").trim().replace(/^@/, "");
      if (!name) return fail("invalid", "Type a Roblox username.");
      if (/nobody/i.test(name)) return fail("invalid", `No Roblox user is called "${name}".`);
      const known = [...accounts, ...FRIENDS].find((x) => x.username.toLowerCase() === name.toLowerCase());
      return known
        ? { userId: known.userId, username: known.username, displayName: known.displayName }
        : { userId: 7000000000 + name.length * 7919, username: name, displayName: name.replace(/_/g, " ") };
    }
    case "launch": {
      await sleep(1200);
      const a = find(args.accountId);
      if (!a) return fail("notFound", "Not found.");
      if (a.username === "afk_unit_1") {
        a.status = "invalid";
        changed();
        return fail("cookieInvalid", "This cookie is not valid or has expired. Sign in again.");
      }
      const f = favourites.find((x) => x.id === args.favouriteId);
      if (f) f.lastUsedAt = Date.now();
      retakeLock();
      const pid = 14000 + Math.floor(Math.random() * 9000);
      track(a.id, pid, args.target, placeOfTarget(args.target), a.username === "studio_nova");
      return {
        accountId: a.id, pid, browserTrackerId: Math.floor(Math.random() * 1e12),
        placeId: placeOfTarget(args.target), startedAt: Date.now(), viaBootstrapper: a.username === "studio_nova",
        warning: args.target.kind === "shareLink" && args.target.code.startsWith("0") ? "This share link has expired, so Roblox may refuse the join." : null,
      };
    }
    case "favourites_list":
      return structuredClone(favourites);
    case "favourite_save": {
      const { id, name, target: t, accountIds, inTray } = args.input;
      const old = favourites.find((x) => x.id === id);
      const f: Favourite = old
        ? Object.assign(old, { name, target: t, accountIds, inTray })
        : { id: Math.max(0, ...favourites.map((x) => x.id)) + 1, name, target: t, accountIds, inTray, sortOrder: favourites.length, lastUsedAt: null };
      if (!old) favourites.push(f);
      return structuredClone(f);
    }
    case "favourite_remove":
      favourites = favourites.filter((x) => x.id !== args.id);
      return null;
    case "favourites_reorder":
      (args.ids as number[]).forEach((id, i) => {
        const f = favourites.find((x) => x.id === id);
        if (f) f.sortOrder = i;
      });
      return null;
    case "join_candidates": {
      await sleep(500);
      if (!find(args.accountId)) return fail("notFound", "Not found.");
      const mine = accounts
        .filter((a) => a.id !== args.accountId && a.status === "valid")
        .map((a): JoinCandidate => ({ userId: a.userId, username: a.username, displayName: a.displayName, accountId: a.id, presence: presenceOf(a) }));
      return structuredClone([...FRIENDS, ...mine]);
    }

    case "groups_list":
      return structuredClone(groups);
    case "group_save": {
      const { id, name, color } = args.input;
      const g = id ? groups.find((x) => x.id === id) : undefined;
      if (g) Object.assign(g, { name, color: color ?? g.color });
      const out = g ?? { id: Math.max(0, ...groups.map((x) => x.id)) + 1, name, color: color ?? null, sortOrder: groups.length };
      if (!g) groups.push(out);
      changed();
      return structuredClone(out);
    }
    case "group_remove":
      groups.splice(groups.findIndex((g) => g.id === args.id), 1);
      accounts.forEach((a) => (a.groupIds = a.groupIds.filter((g) => g !== args.id)));
      changed();
      return null;

    case "backup_export":
      await sleep(600);
      return accounts.length;
    case "backup_import":
      await sleep(800);
      if (args.password !== "password") return fail("wrongPassword", "Wrong password.");
      return { added: 2, updated: args.onConflict === "replace" ? 7 : 0, skipped: args.onConflict === "skip" ? 7 : 0 };

    case "instances_list":
      return structuredClone(instances);
    case "instance_close":
      await sleep(600);
      endInstance(args.id, "Closed");
      return null;
    case "instances_close_all":
      await sleep(900);
      instances.forEach((i) => endInstance(i.id, "Closed"));
      return null;
    case "instances_clear_ended":
      instances = instances.filter((i) => i.state !== "exited");
      instancesChanged();
      return null;
    case "multi_instance_status":
      retakeLock();
      return multiStatus();
    case "multi_instance_set":
      settings.multiInstance = args.enabled;
      if (args.enabled) retakeLock();
      else lockHeld = false;
      return multiStatus();
    case "roblox_close_all":
      await sleep(1200);
      strangerRunning = false;
      instances.forEach((i) => endInstance(i.id, "Closed"));
      retakeLock();
      return multiStatus();

    case "settings_get":
      return { ...settings };
    case "settings_set":
      settings = { ...settings, ...args.patch };
      return { ...settings };

    case "plugin:dialog|save":
      return `C:\\Users\\you\\Documents\\${args.options?.defaultPath ?? "backup.roalt"}`;
    case "plugin:dialog|open":
      return "C:\\Users\\you\\Documents\\RoALT-backup-2026-10-01.roalt";
    case "plugin:window|is_maximized":
      return false;
    default:
      // Window controls, opener and clipboard calls do nothing in the preview.
      if (!/^plugin:(window|opener|clipboard-manager)\|/.test(cmd)) console.warn("[mock ipc] unhandled", cmd);
      return null;
  }
}

mockWindows("main");
mockIPC((cmd, args) => handle(cmd, (args ?? {}) as Args), { shouldMockEvents: true });
