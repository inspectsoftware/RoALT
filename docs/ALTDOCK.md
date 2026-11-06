# AltDock — fork plan and contracts

AltDock is a fork of RoALT (MIT, © 2026 inspectsoftware; see LICENSE and NOTICE.md). It keeps
RoALT's code and adds a **background mode**: Roblox clients run in separate Windows sessions
(loopback RDP through RDP Wrapper), so anti-AFK and AutoHotkey macros never take the user's
focus. This file is the contract every work stream builds against. Code, comments and
identifiers stay in English and in RoALT's style (short `///` docs that say why, `tr!` for
user-facing backend text, ts-rs bindings, one typed wrapper per command in `src/api.ts`, a mock
for every command in `src/dev/mockIpc.ts`).

## Rules for every work stream

- Touch only the files your stream owns (below). Shared files (`model.rs`, `commands.rs`,
  `main.rs`, `Cargo.toml`, `src/api.ts`, `src/dev/mockIpc.ts`, locale files) get **additions in
  one clearly marked block** (`// ---- AltDock: <stream> ----`), never edits elsewhere, so merges
  stay mechanical.
- Prefer putting new types and commands in your own module and only registering them in the
  shared files.
- New UI strings: add English (`src/i18n/locales/en.json`) and German (`de.json`); other
  languages fall back to English.
- Never log or send a cookie, ticket, password or token. Secrets go in the vault (AES-GCM with an
  AAD of their own) or Windows Credential Manager.
- No code may be copied from GPL projects (e.g. BSSaltManager). Ideas yes, code no.
- Windows-only code sits behind `#[cfg(windows)]` like the rest of RoALT.
- Check your work with the cross-check command in "Checking" below when it is available; fix
  every error and warning you introduced.

## Architecture

```
User's session                               Background session(s) (alt Windows users)
┌──────────────────────────────┐   HTTP on   ┌──────────────────────────────────────────┐
│ AltDock (main)               │ 127.0.0.1   │ AltDock (session mode, same exe)         │
│ - Sessions view (new)        │◄───────────►│ - control API server (new, `control.rs`) │
│ - sessions manager (new)     │  token auth │ - unchanged RoALT core: launch, tracker,  │
│ - control API client (new)   │             │   rejoin, previews, multi-instance       │
│ - everything RoALT had       │             │ - macros (restored) + anti-AFK per client│
└──────────────────────────────┘             └──────────────────────────────────────────┘
        │ mstsc.exe (hidden window) per session, RDP Wrapper on the host
```

- **Shared session**: several clients without macros, one alt user, anti-AFK rotates focus there.
- **Dedicated session**: one client per alt user, for a client running a continuous AHK macro.
- The same `altdock.exe` runs in both roles. In a session it starts in **session mode** when a
  control config exists for the signed-in Windows user (see Paths).

## Paths and names

| What | Where |
|---|---|
| Data folder (per Windows user) | `%LOCALAPPDATA%\AltDock` (not `RoALT`: must never touch a real RoALT install) |
| Tauri identifier | `dev.altdock.app` |
| Shared program copy for alt users | `%ProgramData%\AltDock\app\altdock.exe` (+ resources); ACL: Administrators full, main user modify, alt users read/execute |
| Managed alt user list | `%ProgramData%\AltDock\users.json` `{ "users": ["altdock-1", …] }` |
| Control config per alt user | `%ProgramData%\AltDock\control\<windows-user>.json` `{ "port": 47611, "token": "<64 hex>", "mode": "shared"|"dedicated" }`; ACL: that alt user read, main user full, Administrators full |
| Autostart | `HKLM\Software\Microsoft\Windows\CurrentVersion\Run` value `AltDockSession` = `"%ProgramData%\AltDock\app\altdock.exe" --session` — exits at once unless the user is in `users.json` |
| Alt Windows users | `altdock-1`, `altdock-2`, … hidden from the sign-in screen |
| Loopback addresses | `127.0.0.2` for session 1, `127.0.0.3` for session 2, … (one Credential Manager entry `TERMSRV/<addr>` each) |
| Control ports | `47610 + session index` (session 1 → 47611) |

## Stream R — rebrand and privacy (runs first)

Owns: `tauri.conf.json`, `installer/`, `README.md`, `NOTICE.md`, `package.json` name,
`src-tauri/Cargo.toml` package name/authors/description, icons, every "RoALT" UI/log string,
`online.rs`, `news.rs`, `self_update.rs`, `discord.rs` links, `.github/workflows`.

- Product name **AltDock**, identifier `dev.altdock.app`, binary `altdock`, data folder `AltDock`.
- New simple logo (SVG → icon set via `npm run tauri icon` if available, else keep the PNG/ICO
  sizes and draw them). Nothing of RoALT's logo or look-specific branding.
- README: short AltDock README, says it is a fork of RoALT by inspectsoftware (MIT), not
  affiliated, not RoALT. Keep LICENSE unchanged and add our own copyright line *below* theirs.
- Online counter: removed (no request to RoALT's worker). News: only DevForum/Newsroom (no RoALT
  releases). Self-update: disabled (no update source yet; keep the code, point it nowhere, UI hides
  it). Discord buttons/links to RoALT's server: removed.
- Installer and CI keep working with the new names.

## Stream A — macros restored + anti-AFK per client

Owns: `src-tauri/src/macros.rs` (restore from commit `eead0a5`), migration `013_macros_v2.sql`,
`src-tauri/src/instances/afk.rs`, `src/features/macros/*`, client-menu/instance-card additions for
these two features.

Macros (start from 1.0.0's code: one AutoHotkey v2 process per client, script on stdin, window id
and pid as arguments, kill-on-close job, trusted-only, hotkeys):
- **Per account assignment** instead of the global "run on launch": table
  `account_macros(account_id PRIMARY KEY, macro_id, autorun INTEGER)`. When that account's client
  reaches a game and `autorun` is on, its macro starts. Keep manual Run/Stop per client.
- If the macro process exits on its own while the client is still in game, restart it after 5 s,
  at most 3 times in 5 minutes, then give up with a notice.
- Public functions the control API calls (keep these names):
  `macros::list(s) -> Vec<Macro>`, `macros::save(s, MacroInput) -> Macro` (trusted),
  `macros::assign(s, account_id, Option<i64> macro_id, autorun: bool)`,
  `macros::run(app, s, macro_id, &[instance_id])`, `macros::stop_instance(app, instance_id)`,
  `macros::runs(s) -> Vec<MacroRun>`.

Anti-AFK per client:
- Per account settings (survive relaunches): `enabled`, `interval_sec` (60–1140, default from the
  global setting), `action`: `F13 | Space | Jump | JumpTurn | Key(String)` (`Jump` = Space,
  `JumpTurn` = Space then hold Left arrow 0.6 s; `Key` = one key name). Stored with the account
  options. The global setting stays as the default.
- The engine (focus, idle gate, restore focus) stays as it is; only "which key and how often"
  becomes per client.
- Public function: `instances::afk::set_client(app, s, account_id, AfkClientConfig)` and
  `instances::afk::client_config(s, account_id) -> AfkClientConfig`.

## Stream B — control API (session mode)

Owns: `src-tauri/src/control/` (`server.rs`, `client.rs`, `types.rs`, `mod.rs`), the `--session`
start path in `main.rs`, `axum` in `Cargo.toml`.

Start from RoALT 0.6.7's `remote.rs` (commit `b093ddc^`): router, constant-time token compare,
hardening. Drop TLS, PIN, LAN binding, certificates: **bind 127.0.0.1 only**, header
`X-AltDock-Token: <token>` on every request, compared in constant time; `Host` must be
`127.0.0.1:<port>`; JSON only; 1 MB body limit.

Session mode (`altdock.exe --session` or any start where a control config exists for the current
user): start minimized to the tray, skip the splash, multi-instance on, vault in DPAPI mode
(created silently if new), then serve the control API with the config's port and token.
`POST /v1/logoff` ends the session (`ExitWindowsEx(EWX_LOGOFF)`).

Endpoints (all JSON, all need the token; types in `control/types.rs`, exported with ts-rs):

| Method & path | Body → Answer |
|---|---|
| `GET /v1/health` | → `ControlHealth { version, windowsUser, sessionId, mode, vaultLocked, robloxInstalled, clients }` |
| `GET /v1/state` | → `Vec<ControlClient>` (instance id, account id, username, display name, state, place name, started at, cpu %, memory MB, `afk: AfkClientConfig`, `macroRun: Option<MacroRun>`, `macroAssigned: Option<i64>`) |
| `GET /v1/accounts` | → `Vec<ControlAccount { id, username, displayName, status }>` |
| `POST /v1/accounts/import` | `{ backup: base64, password }` → `{ imported }` (uses `vault::backup` reading from bytes) |
| `POST /v1/launch` | `{ accountIds: [i64], target: String }` → `{ started }` (target parsed like the Launch box) |
| `POST /v1/close` | `{ id, force }` |
| `POST /v1/kill-all` | — |
| `GET /v1/thumbnail/{id}?w=320` | → bytes in the same format `instance_thumbnail` returns |
| `POST /v1/afk` | `{ accountId, config: AfkClientConfig }` |
| `GET /v1/macros` | → `Vec<Macro>` |
| `POST /v1/macros` | `MacroInput` → `Macro` (macros pushed from the main app count as trusted) |
| `POST /v1/macro/assign` | `{ accountId, macroId: Option<i64>, autorun }` |
| `POST /v1/macro/run` | `{ macroId, instanceIds }` |
| `POST /v1/macro/stop` | `{ instanceId }` |
| `POST /v1/roblox/install` | starts the existing verified Roblox install for this Windows user |
| `POST /v1/logoff` | — |

Client side (`control/client.rs`): `ControlClient::new(port, token)` with one async method per
endpoint (reqwest, no proxy, 5 s timeout, never follows redirects).

## Stream C — sessions manager (main app)

Owns: `src-tauri/src/sessions/` (`mod.rs`, `health.rs`, `setup.rs`, `users.rs`, `creds.rs`,
`rdp.rs`, `window.rs`, `wts.rs`, `types.rs`), session commands, `Settings.sessions`.

- `SessionConfig { name, windowsUser, address, port, mode: Shared|Dedicated, accountIds: Vec<i64>, width, height }`
  in settings; the alt user's password and the control token live in the vault
  (`AAD altdock.session.<name>.password` / `.token`).
- **Health** (`sessions_health() -> Vec<HealthCheck { id, ok, title, detail, fix: Option<FixKind> }>`):
  RDP Wrapper installed and its service/listener running; `rdpwrap.ini` has a section for the
  exact `termsrv.dll` version (use the numeric file version parts, not the version string);
  `fDenyTSConnections = 0`; `fSingleSessionPerUser = 0`; `RemoteDesktop_SuppressWhenMinimized = 2`
  (HKCU, Terminal Server Client); hardware graphics for RDP sessions (policy
  `bEnumerateHWBeforeSW = 1`); first-logon privacy screen and animation off; shared program copy
  present and same version; autostart value present; alt users exist and are in Remote Desktop
  Users (resolve by SID `S-1-5-32-555`, the name is localized); `.rdp` signing trusted.
  RDP Wrapper itself is never downloaded or installed by AltDock: the check links to its
  instructions.
- **Setup** (one UAC prompt per run): the same exe started elevated with
  `--altdock-setup <plan.json>` through `ShellExecuteExW(runas)` (like `instances/qos.rs`). It
  creates/repairs alt users (`NetUserAdd`, random 24-char password, never on a command line),
  adds them to Remote Desktop Users by SID, hides them from the sign-in screen, sets the policies
  above, copies the program to `%ProgramData%\AltDock\app` with the ACLs in Paths, writes
  `users.json` and the control configs with their ACLs, registers the autostart, makes a
  code-signing certificate (LocalMachine\My + Root) and lists its thumbprint in
  `TrustedCertThumbprints`, and signs each `.rdp` with `rdpsign.exe /sha256 <thumbprint>`.
  It writes a result JSON next to the plan; the main app reads it.
- **Credentials**: `CredWriteW` `TERMSRV/<address>` (`CRED_TYPE_DOMAIN_PASSWORD`) in the main
  user's store — no elevation.
- **.rdp file** (main data folder, `sessions\<name>.rdp`): `full address`, `username`, fixed
  `desktopwidth/height`, `session bpp:i:32`, `smart sizing:i:0`, `dynamic resolution:i:0`,
  `screen mode id:i:1`, `audiomode:i:2`, no drive/printer/clipboard redirection,
  `autoreconnection enabled:i:1`, `prompt for credentials:i:0`, `authentication level:i:0`.
- **Window**: start `mstsc.exe <file>`; find its window (class `TscShellContainerClass`, title
  contains the address); **hide** it (`SW_HIDE`), never minimize. `show`/`hide` toggle.
- **State**: `WTSEnumerateSessionsW` + `WTSQuerySessionInformationW(WTSUserName)` → active /
  disconnected / none per alt user.
- Before `session_start`: refuse if the RDP Wrapper version check fails (a mismatched wrapper
  takes over the user's own session).
- Commands for the UI:
  `sessions_health`, `sessions_setup(names)`, `sessions_list() -> Vec<SessionView>`,
  `session_save(config)`, `session_remove(name, deleteUser)`, `session_start(name)`,
  `session_window(name, show)`, `session_disconnect(name)`, `session_logoff(name)` (through the
  control API), `session_push_accounts(name, accountIds)` (encrypted backup with a random one-time
  password → `POST /v1/accounts/import`), `session_push_macros(name, macroIds)`, and thin
  proxies to the control API: `remote_state`, `remote_thumbnail`, `remote_launch`,
  `remote_close`, `remote_kill_all`, `remote_afk`, `remote_macro_assign`, `remote_macro_run`,
  `remote_macro_stop`, `remote_roblox_install`.
- `SessionView { config, wts: Active|Disconnected|None, windowVisible, control: Ok{health}|Down(String) }`.

## Stream D — Sessions UI

Owns: `src/features/sessions/*`, its nav entry, its parts of `src/api.ts`, `src/dev/mockIpc.ts`
and the locale files.

- **Sessions** page: health panel (each check with its fix, one "Set up" button that runs the
  elevated setup), session list (state, start, show/hide, disconnect, log off, edit, remove),
  "Add session" dialog (name, mode shared/dedicated, accounts, size).
- Per session: client cards with live thumbnails (`remote_thumbnail`, ~1/s while visible), state,
  close, and per client: anti-AFK (on/off, interval, action) and macro (assigned macro, autorun,
  run/stop, running state). "Push accounts" and "Push macros" buttons. Launch box (target + the
  session's accounts).
- Reuse the dashboard's card look (`features/dashboard/ClientCard.tsx` styles) where it fits.
- Works in `npm run dev` against mocks for every new command.

## Integrated API (after the merge)

This is what the code does now; where it differs from the stream sections above, this wins.

**Types** (Rust is the source of truth; every `#[ts(export)]` type has its file in `src/bindings/`):
`control::types` (`ControlHealth`, `ControlMode`, `ControlClient`, `ControlAccount`, the request and
answer bodies), `model` (Stream A: `Macro`, `MacroInput`, `MacroRun`, `MacroAssignment`,
`AfkAction`, `AfkClientConfig`), `sessions::types` (`SessionConfig`, `SessionMode`, `SessionView`,
`SessionControl = {kind:"ok",health} | {kind:"down",reason}`, `WtsState`, `HealthCheck`,
`FixKind = setup | rdpWrapper | rdpWrapperIni | restart`, `SetupResult`, `SetupStep`).

**Account ids**: a session has its own database. `ControlClient` and `ControlAccount` carry the
session's `accountId` and the Roblox `userId`; the UI matches its own accounts by `userId`.
Commands fed from the session's answers (`remote_afk`, `remote_macro_assign`, `remote_macro_run`,
`remote_macro_stop`, `remote_close`) take the session's ids; `remote_launch` and
`session_push_accounts` take the main app's ids (`remote_launch` maps them by Roblox user and
refuses accounts not pushed yet).

**`GET /v1/state`** rows also have `userId` and `macroAutorun` (the assigned macro's autorun flag;
false without one); `afk` is `instances::afk::client_config`, `macroRun` from `macros::runs`.
`POST /v1/macros` is `macros::save` then `macros::sync_hotkeys`; `POST /v1/macro/assign` emits
`macros://assignments` in the session. Session mode forces `multi_instance`, `close_to_tray` and
the global `anti_afk.enabled` on (each client's own `AfkClientConfig.enabled` decides).

**Client** (`control::client::ControlClient`): `new(port, token)` (one shared reqwest client, no
proxy, no redirects, 5 s; import/close/kill-all 120 s), `health(wait)`, `state`, `accounts`,
`import_accounts(&[u8], password)`, `launch`, `close`, `kill_all`, `thumbnail(id, w, known)`, `afk`,
`macros`, `macro_save`, `macro_assign`, `macro_run`, `macro_stop`, `roblox_install`, `logoff`.

**Commands** (`src/api.ts` has one wrapper each, `src/dev/mockIpc.ts` one mock each):

| Command | Args → answer |
|---|---|
| `sessions_health` | → `HealthCheck[]` |
| `sessions_setup` | `names` (empty: all) → `SetupResult` |
| `sessions_list` | → `SessionView[]`; every session's `/v1/health` asked in parallel, ≤ 1.5 s each |
| `session_save` | `config` → `SessionConfig` (slot assigned or kept) |
| `session_remove` | `name, deleteUser` → `SetupResult \| null` |
| `session_start` / `session_disconnect` | `name` |
| `session_window` | `name, show` → shown now (`bool`) |
| `session_logoff` | `name` |
| `session_push_accounts` | `name, accountIds` (main ids) → `ImportAnswer { imported, failed }` |
| `session_push_macros` | `name, macroIds` → count; only trusted macros, without hotkeys; same name replaced |
| `remote_state` / `remote_macros` | `name` → `ControlClient[]` / `Macro[]` (session ids) |
| `remote_thumbnail` | `name, id, maxW, known` → bytes as `instance_thumbnail` |
| `remote_launch` | `name, accountIds` (main ids), `target` → `LaunchAnswer { started, batchId }` |
| `remote_close` | `name, id, force` |
| `remote_kill_all` / `remote_roblox_install` | `name` |
| `remote_afk` | `name, accountId, config: AfkClientConfig` |
| `remote_macro_assign` | `name, accountId, macroId \| null, autorun` |
| `remote_macro_run` / `remote_macro_stop` | `name, macroId, instanceIds` / `name, instanceId` |

**Still needs a real Windows run** (only compiled for `x86_64-pc-windows-gnu` and tested against
mocks here): the elevated setup end to end (users, ACLs, policies, certificate, `rdpsign`), RDP
Wrapper version check and hidden mstsc window, session-mode start from the HKLM Run value, the
control API over loopback between two sessions (token, `Host` check, push of accounts and macros,
thumbnails from a session without a console), anti-AFK and AutoHotkey macros inside a disconnected
or hidden session (focus, `SendInput`, rendering while hidden), `ExitWindowsEx` logoff, and the
`opener` permission for the RDP Wrapper link.

## Checking

The cloud workspace can't run Windows. Rust for Windows is checked with:

```
cd src-tauri && RUSTC_BOOTSTRAP=1 CARGO_TARGET_DIR=/home/claude/target-win cargo check -Zbuild-std --target x86_64-pc-windows-gnu --all-targets
```

(and the same with `clippy`). The UI is checked with `npm run build` (tsc + i18n check + vite).
`npm run bindings` needs Windows; on Linux the bindings come from a scratch crate that includes
`control/types.rs`, `sessions/types.rs` and model's Stream A block and runs ts-rs' export tests.
