// Dev-only fake backend so the UI can be previewed in a normal browser (`npm run dev`).
// main.tsx loads it only when import.meta.env.DEV and there is no Tauri runtime.
// Master password for the fake vault: "password". A pasted cookie containing "bad" is rejected.
import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { AccountStatus, AccountSummary, Group, Settings, VaultMode } from "../api";

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

let settings: Settings = { theme: "dark", density: "detailed", autoLockMinutes: 15, clipboardClearSeconds: 30 };
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
