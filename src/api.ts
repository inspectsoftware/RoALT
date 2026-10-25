// Typed wrappers for every Tauri command and event. Types come from src/bindings (ts-rs).
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AccountInfo } from "./bindings/AccountInfo";
import type { AccountPatch } from "./bindings/AccountPatch";
import type { AccountPresence } from "./bindings/AccountPresence";
import type { AccountSummary } from "./bindings/AccountSummary";
import type { BulkProgress } from "./bindings/BulkProgress";
import type { BulkRequest } from "./bindings/BulkRequest";
import type { ConflictPolicy } from "./bindings/ConflictPolicy";
import type { ErrorPayload } from "./bindings/ErrorPayload";
import type { Favourite } from "./bindings/Favourite";
import type { FavouriteInput } from "./bindings/FavouriteInput";
import type { GameInfo } from "./bindings/GameInfo";
import type { Group } from "./bindings/Group";
import type { GroupInput } from "./bindings/GroupInput";
import type { Headshot } from "./bindings/Headshot";
import type { ImportReport } from "./bindings/ImportReport";
import type { Instance } from "./bindings/Instance";
import type { JoinCandidate } from "./bindings/JoinCandidate";
import type { LaunchInfo } from "./bindings/LaunchInfo";
import type { LaunchTarget } from "./bindings/LaunchTarget";
import type { MultiInstanceStatus } from "./bindings/MultiInstanceStatus";
import type { ParsedInput } from "./bindings/ParsedInput";
import type { PrivateServerPage } from "./bindings/PrivateServerPage";
import type { ServerOrder } from "./bindings/ServerOrder";
import type { ServerPage } from "./bindings/ServerPage";
import type { Settings } from "./bindings/Settings";
import type { SettingsPatch } from "./bindings/SettingsPatch";
import type { UserRef } from "./bindings/UserRef";
import type { VaultMode } from "./bindings/VaultMode";
import type { VaultStatus } from "./bindings/VaultStatus";

export type {
  AccountInfo, AccountPresence, Headshot,
  Favourite, FavouriteInput, JoinCandidate, LaunchInfo, LaunchTarget, ParsedInput, UserRef,
  Instance, MultiInstanceStatus, BulkProgress, BulkRequest, ServerOrder, ServerPage, GameInfo, PrivateServerPage,
  AccountPatch, AccountSummary, ConflictPolicy, ErrorPayload, Group, GroupInput,
  ImportReport, Settings, SettingsPatch, VaultMode, VaultStatus,
};
export type { AccountStatus } from "./bindings/AccountStatus";
export type { BulkItem } from "./bindings/BulkItem";
export type { BulkStage } from "./bindings/BulkStage";
export type { Density } from "./bindings/Density";
export type { GameServer } from "./bindings/GameServer";
export type { PrivateServer } from "./bindings/PrivateServer";
export type { SameServer } from "./bindings/SameServer";
export type { InstanceState } from "./bindings/InstanceState";
export type { Presence } from "./bindings/Presence";
export type { PresenceKind } from "./bindings/PresenceKind";
export type { Subscription } from "./bindings/Subscription";
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

  /** Cached for settings.cacheMinutes unless refresh. Parts Roblox didn't answer are null and named in `unavailable`. */
  accountInfo: (id: number, refresh = false) => invoke<AccountInfo>("account_info", { id, refresh }),
  /** Avatar URLs for all accounts (cached 6 h); accounts without one are missing from the list. */
  accountsHeadshots: (refresh = false) => invoke<Headshot[]>("accounts_headshots", { refresh }),
  /** Presence of these accounts, each asked with its own cookie (accounts without a valid cookie are left out). */
  accountsPresence: (ids: number[]) => invoke<AccountPresence[]>("accounts_presence", { ids }),
  /** Opens roblox.com signed in as the account in its own window. */
  accountOpenSession: (id: number) => invoke<void>("account_open_session", { id }),

  /** Local, instant: what pasted text points at. */
  targetParse: (input: string) => invoke<ParsedInput>("target_parse", { input }),
  /** A Roblox username (current or previous) to its user. */
  userLookup: (name: string) => invoke<UserRef>("user_lookup", { name }),
  /** Starts the Roblox client as this account. */
  launch: (accountId: number, target: LaunchTarget, favouriteId?: number) =>
    invoke<LaunchInfo>("launch", { accountId, target, favouriteId }),
  favouritesList: () => invoke<Favourite[]>("favourites_list"),
  favouriteSave: (input: FavouriteInput) => invoke<Favourite>("favourite_save", { input }),
  favouriteRemove: (id: number) => invoke<void>("favourite_remove", { id }),
  favouritesReorder: (ids: number[]) => invoke<void>("favourites_reorder", { ids }),
  /** Friends online plus the other stored accounts, for "Join user". */
  joinCandidates: (accountId: number) => invoke<JoinCandidate[]>("join_candidates", { accountId }),

  instancesList: () => invoke<Instance[]>("instances_list"),
  /** Graceful unless `force`; not counted as an unexpected exit. */
  instanceClose: (id: string, force: boolean) => invoke<void>("instance_close", { id, force }),
  instancesCloseAll: (force: boolean) => invoke<void>("instances_close_all", { force }),
  instancesClearEnded: () => invoke<void>("instances_clear_ended"),
  multiInstanceStatus: () => invoke<MultiInstanceStatus>("multi_instance_status"),
  multiInstanceSet: (enabled: boolean) => invoke<MultiInstanceStatus>("multi_instance_set", { enabled }),
  /** Every Roblox client, tracked or not; then takes the multi-instance lock if enabled. */
  robloxCloseAll: (force: boolean) => invoke<MultiInstanceStatus>("roblox_close_all", { force }),

  /** Starts a batch in the background; progress comes as bulk://progress. Returns its id. */
  bulkLaunch: (request: BulkRequest) => invoke<string>("bulk_launch", { request }),
  bulkCancel: (batchId: string) => invoke<void>("bulk_cancel", { batchId }),
  bulkCurrent: () => invoke<BulkProgress | null>("bulk_current"),
  serversList: (placeId: number, order: ServerOrder, excludeFull: boolean, limit: 10 | 25 | 50 | 100, cursor?: string, accountId?: number) =>
    invoke<ServerPage>("servers_list", { placeId, order, excludeFull, limit, cursor, accountId }),
  gameInfo: (placeId: number) => invoke<GameInfo>("game_info", { placeId }),
  privateServersList: (placeId: number, accountId: number, cursor?: string) =>
    invoke<PrivateServerPage>("private_servers_list", { placeId, accountId, cursor }),

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
  onInstancesChanged: (f: () => void): Promise<UnlistenFn> => listen("instances://changed", f),
  onBulkProgress: (f: (p: BulkProgress) => void): Promise<UnlistenFn> => listen<BulkProgress>("bulk://progress", (e) => f(e.payload)),
};
