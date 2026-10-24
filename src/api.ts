// Typed wrappers for every Tauri command and event. Types come from src/bindings (ts-rs).
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AccountPatch } from "./bindings/AccountPatch";
import type { AccountSummary } from "./bindings/AccountSummary";
import type { ConflictPolicy } from "./bindings/ConflictPolicy";
import type { ErrorPayload } from "./bindings/ErrorPayload";
import type { Group } from "./bindings/Group";
import type { GroupInput } from "./bindings/GroupInput";
import type { ImportReport } from "./bindings/ImportReport";
import type { Settings } from "./bindings/Settings";
import type { SettingsPatch } from "./bindings/SettingsPatch";
import type { VaultMode } from "./bindings/VaultMode";
import type { VaultStatus } from "./bindings/VaultStatus";

export type {
  AccountPatch, AccountSummary, ConflictPolicy, ErrorPayload, Group, GroupInput,
  ImportReport, Settings, SettingsPatch, VaultMode, VaultStatus,
};
export type { AccountStatus } from "./bindings/AccountStatus";
export type { Density } from "./bindings/Density";
export type { Theme } from "./bindings/Theme";

/** Rejected invokes carry an ErrorPayload; anything else becomes an "internal" one. */
export function asError(e: unknown): ErrorPayload {
  if (e && typeof e === "object" && "kind" in e && "message" in e) return e as ErrorPayload;
  return { kind: "internal", message: String(e) };
}

export const api = {
  vaultStatus: () => invoke<VaultStatus>("vault_status"),
  vaultUnlock: (password: string) => invoke<void>("vault_unlock", { password }),
  vaultLock: () => invoke<void>("vault_lock"),
  vaultSetMode: (mode: VaultMode, currentPassword?: string, newPassword?: string) =>
    invoke<void>("vault_set_mode", { mode, currentPassword, newPassword }),
  appActivity: () => invoke<void>("app_activity"),

  accountsList: () => invoke<AccountSummary[]>("accounts_list"),
  accountAddCookie: (cookie: string) => invoke<AccountSummary>("account_add_cookie", { cookie }),
  /** Opens the Roblox sign-in window; resolves once the cookie is captured. */
  accountAddLogin: () => invoke<AccountSummary>("account_add_login"),
  accountUpdate: (id: number, patch: AccountPatch) => invoke<AccountSummary>("account_update", { id, patch }),
  accountsReorder: (ids: number[]) => invoke<void>("accounts_reorder", { ids }),
  accountsRemove: (ids: number[]) => invoke<void>("accounts_remove", { ids }),
  accountsValidate: (ids?: number[]) => invoke<AccountSummary[]>("accounts_validate", { ids }),
  /** Master mode needs the password; DPAPI mode shows Windows Hello. */
  accountRevealCookie: (id: number, password?: string) => invoke<string>("account_reveal_cookie", { id, password }),
  accountCopyCookie: (id: number, password?: string) => invoke<void>("account_copy_cookie", { id, password }),

  groupsList: () => invoke<Group[]>("groups_list"),
  groupSave: (input: GroupInput) => invoke<Group>("group_save", { input }),
  groupRemove: (id: number) => invoke<void>("group_remove", { id }),

  /** Master mode needs the master password (currentPassword); DPAPI mode shows Windows Hello. */
  backupExport: (path: string, password: string, currentPassword?: string, accountIds?: number[]) =>
    invoke<number>("backup_export", { path, password, currentPassword, accountIds }),
  backupImport: (path: string, password: string, onConflict: ConflictPolicy) =>
    invoke<ImportReport>("backup_import", { path, password, onConflict }),

  settingsGet: () => invoke<Settings>("settings_get"),
  settingsSet: (patch: SettingsPatch) => invoke<Settings>("settings_set", { patch }),
};

export const events = {
  onAccountsChanged: (f: () => void): Promise<UnlistenFn> => listen("accounts://changed", f),
  onVaultLocked: (f: () => void): Promise<UnlistenFn> => listen("vault://locked", f),
};
