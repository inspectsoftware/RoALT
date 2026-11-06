// TODO(merge): replace with generated bindings
// Hand-written mirrors of Stream B's `control/types.rs` and Stream C's `sessions/types.rs`
// (docs/ALTDOCK.md), camelCase as ts-rs exports them. Once those land, import from src/bindings
// instead and delete this file.
import type { InstanceState } from "../../bindings/InstanceState";

/** Shared: several clients without macros under one alt user. Dedicated: one client, for a
 * continuous macro. */
export type SessionMode = "shared" | "dedicated";

/** One background session as kept in settings (`Settings.sessions`). */
export type SessionConfig = {
  name: string;
  windowsUser: string;
  /** Loopback address the RDP client connects to (127.0.0.2, …). */
  address: string;
  /** Control API port (47610 + session index). */
  port: number;
  mode: SessionMode;
  accountIds: number[];
  /** The session's desktop size. */
  width: number;
  height: number;
};

/** The alt user's Windows session (`WTSEnumerateSessionsW`). */
export type WtsState = "active" | "disconnected" | "none";

/** `GET /v1/health` of AltDock running in the session. */
export type ControlHealth = {
  version: string;
  windowsUser: string;
  sessionId: number;
  mode: SessionMode;
  vaultLocked: boolean;
  robloxInstalled: boolean;
  /** How many clients run there. */
  clients: number;
};

/** The session's control API: answering with its health, or why not. */
export type ControlStatus = { ok: true; health: ControlHealth } | { ok: false; error: string };

export type SessionView = {
  config: SessionConfig;
  wts: WtsState;
  /** The hidden mstsc window is shown. */
  windowVisible: boolean;
  control: ControlStatus;
};

/** What anti-AFK presses: `jump` = Space, `jumpTurn` = Space then Left arrow held 0.6 s, `key`: one key name. */
export type AfkAction = "f13" | "space" | "jump" | "jumpTurn" | { key: string };

/** Anti-AFK for one account's client (survives relaunches). */
export type AfkClientConfig = {
  enabled: boolean;
  /** 60–1140. */
  intervalSec: number;
  action: AfkAction;
};

/** A stored macro, as far as the sessions UI needs it. */
export type Macro = { id: number; name: string };

/** A macro running on one client (one AutoHotkey process). */
export type MacroRun = {
  instanceId: string;
  macroId: number;
  name: string;
  /** Unix ms. */
  startedAt: number;
};

/** One client running in a session (`GET /v1/state`). */
export type ControlClient = {
  instanceId: string;
  accountId: number;
  username: string;
  displayName: string;
  state: InstanceState;
  placeName: string | null;
  /** Unix ms. */
  startedAt: number;
  /** Percent of the PC; null before the first sample. */
  cpu: number | null;
  memoryMb: number | null;
  afk: AfkClientConfig;
  macroRun: MacroRun | null;
  /** The macro assigned to its account (in the session's own macro list). */
  macroAssigned: number | null;
  /** Whether the assigned macro starts when the client reaches a game.
   * TODO(merge): not in the contract's `ControlClient`; the toggle needs it (Stream B to add).
   * Until then the UI keeps the last choice made here. */
  macroAutorun?: boolean;
};

/** How a failing check gets fixed: by the elevated "Set up", by following instructions (RDP
 * Wrapper is never installed by AltDock), or by hand as the text says.
 * TODO(merge): the contract names `FixKind` without its variants; match Stream C's enum. */
export type FixKind = "setup" | { link: string } | { manual: string };

/** One line of `sessions_health`. */
export type HealthCheck = {
  id: string;
  ok: boolean;
  title: string;
  detail: string;
  fix: FixKind | null;
};
