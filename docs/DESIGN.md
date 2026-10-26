# RoALT — design proposal (for approval)

Status: proposal, 2026-10-24. Nothing is built yet. Roblox behaviour below was verified on 2026-10-24
against the live roblox.com JS bundles, Roblox's published OpenAPI, unauthenticated probes, the local
client (0.744) and its logs, and maintained open-source tools. Raw research with sources:
`docs/research/*.json`. UI reference: https://claude.ai/artifact/QghK57bscp6nd4AswoYcMT

## 1. Stack

Tauri 2.12 (Rust) + React 19 + TypeScript + Vite + Tailwind 4 + TanStack Query 5, SQLite via rusqlite.
No reason to switch. Two deviations from the brief:

- **Typed IPC with ts-rs, not tauri-specta.** tauri-specta for Tauri 2 is still a release candidate
  (2.0.0-rc.26) that needs exact version pins and has broken builds between RCs. ts-rs 12 generates the
  shared types; `src/api.ts` holds one typed wrapper per command (a few lines each).
- **npm** as the package manager (pnpm is not installed; `bunx --bun` breaks the Tauri CLI).

Pinned: tauri 2.12.5, @tauri-apps/cli 2.12.4, rusqlite 0.40.2 (bundled) + rusqlite_migration 2.6,
reqwest 0.13 (rustls, no cookie jar), windows 0.62, webview2-com 0.39, argon2 0.6, aes-gcm 0.11,
zeroize/secrecy, ts-rs 12, tokio ≥ 1.54.1 (1.54.0 is yanked). Plugins: single-instance, opener,
window-state, dialog. Tray via the `tray-icon` feature.

## 2. What differs from the brief (verified)

| Brief assumed | Reality on 2026-10-24 | Design consequence |
|---|---|---|
| Get CSRF by POSTing an auth endpoint | Without a cookie, auth endpoints return 401 before any CSRF check; `/v2/logout` with a valid token **ends the session**. The real request itself returns 403 + `x-csrf-token`. | Send the real request; on 403 with `x-csrf-token`, store it per account and retry once. Never call logout. |
| POST `authentication-ticket` with cookie + CSRF + Referer | Website now does `GET auth.roblox.com/v1/client-assertion/`, then POSTs the ticket request with that JSON as the body (`{}` if the GET fails). | Mirror the website exactly. |
| PlaceLauncher on assetgame | Website uses `https://www.roblox.com/Game/PlaceLauncher.ashx`, adds `joinAttemptId` + `joinAttemptOrigin`; URI ends `+LaunchExp:InApp`, no `channel`. Private: `linkCode` (and/or `accessCode`). | URI builder byte-matches the website; tested against captured examples. |
| Share link → placeId + linkCode | `POST apis.roblox.com/sharelinks/v1/resolve-link {linkId, linkType}` needs cookie + CSRF. Server links return `privateServerInviteData {status, placeId, universeId, linkCode (string), ownerUserId, privateServerId}`. A second format (`?v=v2`) uses `apis.roblox.com/deeplinks/v2/resolve`. | Resolve per account at launch with that account's cookie; `Expired` proceeds with a warning, as the website does. Both formats supported. |
| Hold `ROBLOX_singletonMutex` | Stopped working mid-2025 (Bloxstrap removed multi-instance in Oct 2025). The current client (0.744) creates a named **event** `ROBLOX_singletonEvent` and signals it to make the old client quit. A **mutex with that exact name** (case-sensitive, no prefix) created before the first client starts blocks that. Roblox's tray process, auto-started from `HKCU\…\Run`, can already hold the name. | Create a non-owned mutex `ROBLOX_singletonEvent` at startup when multi-instance is on. If the name is taken (`ERROR_INVALID_HANDLE`), a Roblox process is running: say so and offer to close it. Do not touch `ROBLOX_singletonMutex` (clients wait on it during handover). |
| Clients are independent | All clients share `%LOCALAPPDATA%\Roblox\LocalStorage\RobloxCookies.dat` and `appStorage.json`; this causes teleport error 773 and wrong-account state across clients. Every launch also leaves your normal Roblox app signed in as the last-launched account. | "Protect shared login" (on by default with multi-instance): hold both files with share mode 0 while RoALT's clients run. |
| Cookie is static | Since 2026 Roblox rotates `.ROBLOSECURITY` via `Set-Cookie` on ordinary API responses; old values are rejected months later. | Every response is checked; a rotated cookie is re-encrypted and saved atomically. |
| Plain cookie works anywhere | Hardware-backed auth (HBA / `x-bound-auth-token`) is live on the website. Roblox staff (2026-05-27): cookie workflows are "unaffected"; Account Session Protection is now a background risk signal and its opt-out was removed. Expect challenges, not hard rejections. | Send unsigned; map `401 9002/2` to "Roblox requires a bound session" and `rblx-challenge-*` 403s to "verify in the browser" (opens the account's session window). |
| Login is username + password | Enhanced Protection (2026-04) forces passkeys / security keys on opted-in accounts; the web Account Switcher can swap accounts inside one browser profile. | Login only through the real roblox.com page in WebView2 (WebAuthn works there). After any session window closes, re-check the cookie still belongs to the same user ID. |
| Premium via premiumfeatures | Premium stopped selling 2026-04-30 and its profile badge ended 2026-05-30 (Roblox Plus replaced it). `validate-membership` is marked for deprecation. | One "Subscription" field: Plus from `user-profile-api get-profiles` (`hasRobloxSubscription`), "Premium (legacy)" only if the old endpoint still says so. |
| Robux at `/v1/users/{id}/currency` | Documented endpoint is `GET economy.roblox.com/v1/user/currency`. | Use the documented one. |
| Presence gives last online | `lastOnline` and `/presence/last-online` are gone. Job ID only visible to permitted viewers. | Same-server detection reads the client's own log first, presence second. |
| Server list freely pageable | `limit` must be 10/25/50/100; **3 requests per minute per IP** without a cookie, on a window aligned to the clock minute; website uses `/v2/.../servers/Public?orderBy=OccupancyDesc\|OccupancyAsc\|Recommended\|BestLatency&excludeFullGames=true` (the last two need a cookie). Pages repeat servers. Staff have announced a logged-in-only server list. | Use the selected account's cookie, page on demand, de-duplicate by job ID, wait on `x-ratelimit-reset` (Retry-After understates it). |
| Cookie APIs for any client | Roblox (2026-04-15): cookie-based APIs are "for … usage by humans only", with lower per-endpoint limits since 2026-05-04. | No background crawling. Refreshes are user-triggered or slow (presence ≤ 1/min, visible rows only). |

Multi-instance: Roblox's official statement (2025-05-06, "An Update on Automated Action Against Modified
Clients") says it does **not officially support** several clients at once, and that methods which do not
modify the client binary are not affected by its modified-client enforcement. RoALT only creates a named
object before Roblox starts and never touches the client, but Roblox has broken multi-instance before
(mid-2025). The app shows this once when the toggle is turned on.

Roblox launch tickets appear in plain text in Roblox's own log files and in the client's command line.
RoALT redacts `gameinfo:` from anything it reads and never exports those logs.

## 3. Folder structure

```
QuikSwitch/                      repo root (product name RoALT)
├─ package.json  vite.config.ts  tsconfig.json  index.html
├─ src/                          React UI
│  ├─ main.tsx  App.tsx          window shell: custom title bar, nav, view switch
│  ├─ api.ts                     typed invoke + event wrappers, one per command
│  ├─ bindings/                  ts-rs generated types (committed)
│  ├─ theme.css                  tokens from the design canvas: radius 0, dark/light, motion
│  ├─ ui/                        Button, Switch, Segmented, Table, Dialog, Toast, Kbd
│  └─ features/
│     ├─ accounts/               list, filters, groups, add dialog, detail panel
│     ├─ launch/                 target box, favourites, bulk progress
│     ├─ instances/              running clients
│     ├─ servers/                server browser
│     ├─ settings/
│     └─ vault/                  unlock screen, master-password dialogs
├─ src-tauri/
│  ├─ Cargo.toml  build.rs  tauri.conf.json  capabilities/main.json  icons/
│  ├─ tests/fixtures/            recorded Roblox responses (never real cookies)
│  └─ src/
│     ├─ main.rs  lib.rs         builder, plugins, managed state, tray
│     ├─ error.rs                AppError (thiserror) → {kind, message} for the UI
│     ├─ commands.rs             thin IPC layer (split per feature if it grows)
│     ├─ roblox_api/             the one typed client
│     │  ├─ mod.rs               Client: throttle, retries, CSRF, cookie rotation, error mapping
│     │  ├─ endpoints.rs         every Roblox URL in one table
│     │  ├─ users.rs presence.rs economy.rs friends.rs thumbnails.rs games.rs sharelinks.rs
│     │  └─ types.rs             serde response types (lenient: unknown fields ignored)
│     ├─ auth.rs                 client-assertion → authentication-ticket
│     ├─ launcher/
│     │  ├─ target.rs            link parser (38 recorded cases) and LaunchTarget
│     │  ├─ uri.rs               roblox-player: URI + PlaceLauncher URL
│     │  └─ bulk.rs              batch runner, stagger, same-server, cancel
│     ├─ instances/
│     │  ├─ mod.rs               tracker (PID + creation time), close, auto-rejoin
│     │  ├─ singleton.rs         ROBLOX_singletonEvent mutex + shared-login file lock (Windows)
│     │  ├─ logwatch.rs          tails Roblox client logs: job ID, user ID, disconnects
│     │  └─ platform.rs          ClientPlatform trait; Windows impl
│     ├─ vault/
│     │  ├─ mod.rs               data key, lock/unlock, auto-lock, re-auth
│     │  ├─ dpapi.rs  master.rs  key wrapping
│     │  └─ backup.rs            .roalt export/import
│     ├─ webview.rs              login capture + per-account session windows (temp WebView2 profiles)
│     └─ storage/
│        ├─ mod.rs               one Mutex<Connection>, used via spawn_blocking
│        └─ migrations/001_init.sql
├─ installer/RoALT.iss           Inno Setup script
├─ scripts/build-installer.ps1
├─ .github/workflows/ci.yml  release.yml
└─ docs/DESIGN.md  docs/research/
```

## 4. Data model

### Files on disk

Default data folder `%LOCALAPPDATA%\RoALT` (DPAPI keys are per machine and user, so Local, not Roaming):
`roalt.db`, `logs\`, `webview\` (UI only, never used for roblox.com), `tmp-webview\<uuid>\` (login and
session windows, deleted after use, swept on start). A moved data folder is recorded in
`%LOCALAPPDATA%\RoALT\location.json`. `ROALT_DATA_DIR` overrides everything, for test runs on a hidden desktop.

### Vault (envelope encryption)

- One random 256-bit **data key** encrypts every secret with AES-256-GCM (random 96-bit nonce per value,
  AAD = row identity, so blobs cannot be swapped between rows).
- **Default mode:** data key wrapped with DPAPI (current user, `CRYPTPROTECT_UI_FORBIDDEN`). Opens silently.
- **Master-password mode:** key-encryption key from Argon2id (64 MiB, t=3, p=4, 16-byte salt) wraps the
  data key, and the result is DPAPI-wrapped too, so a copied database cannot be brute-forced off this
  Windows account. Auto-lock after idle zeroes the data key in memory.
- Switching mode only re-wraps the data key. No cookie is re-encrypted.
- Reveal / copy cookie: master mode re-asks the password; DPAPI mode asks Windows Hello. Copy clears the
  clipboard after 30 s. Cookies, tickets, link codes and process command lines never reach logs.

### SQLite schema v1

```sql
CREATE TABLE vault (
  id           INTEGER PRIMARY KEY CHECK (id = 1),
  mode         TEXT    NOT NULL CHECK (mode IN ('dpapi', 'master')),
  wrapped_key  BLOB    NOT NULL,
  kdf_salt     BLOB,                       -- master mode only
  kdf_m_kib    INTEGER,
  kdf_t        INTEGER,
  kdf_p        INTEGER
);

CREATE TABLE accounts (
  id                INTEGER PRIMARY KEY,
  user_id           INTEGER NOT NULL UNIQUE,      -- Roblox user id
  username          TEXT    NOT NULL,
  display_name      TEXT    NOT NULL,
  alias             TEXT    NOT NULL DEFAULT '',
  notes             TEXT    NOT NULL DEFAULT '',
  color             TEXT,                          -- '#rrggbb'
  favourite         INTEGER NOT NULL DEFAULT 0,
  sort_order        INTEGER NOT NULL,
  cookie            BLOB    NOT NULL,              -- nonce || AES-GCM(data key, .ROBLOSECURITY)
  status            TEXT    NOT NULL DEFAULT 'unknown'
                    CHECK (status IN ('unknown', 'valid', 'invalid', 'challenged')),
  status_checked_at INTEGER,                       -- unix ms
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL
);

CREATE TABLE groups (                              -- "tags/groups" are one concept
  id         INTEGER PRIMARY KEY,
  name       TEXT    NOT NULL UNIQUE COLLATE NOCASE,
  color      TEXT,
  sort_order INTEGER NOT NULL
);

CREATE TABLE account_groups (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  group_id   INTEGER NOT NULL REFERENCES groups(id)   ON DELETE CASCADE,
  PRIMARY KEY (account_id, group_id)
) WITHOUT ROWID;

CREATE TABLE favourites (
  id           INTEGER PRIMARY KEY,
  name         TEXT    NOT NULL,
  target       BLOB    NOT NULL,                   -- encrypted LaunchTarget JSON (link codes are bearer secrets)
  account_ids  TEXT    NOT NULL DEFAULT '[]',      -- default accounts for one-click / tray launch
  in_tray      INTEGER NOT NULL DEFAULT 1,
  sort_order   INTEGER NOT NULL,
  last_used_at INTEGER
);

CREATE TABLE cache (                               -- API cache; TTL decided by key prefix
  key        TEXT    PRIMARY KEY,                  -- 'profile:<uid>', 'robux:<uid>', 'universe:<placeId>', ...
  value      TEXT    NOT NULL,                     -- JSON
  fetched_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL                              -- JSON
) WITHOUT ROWID;
```

Running instances and bulk batches live in memory only (PIDs mean nothing after a restart).

### Core types (Rust, exported to TS)

```rust
enum LaunchTarget {
    Place         { place_id: u64, launch_data: Option<String> },
    Server        { place_id: u64, job_id: Uuid },
    PrivateLink   { place_id: u64, link_code: String },
    PrivateAccess { place_id: u64, access_code: Uuid },
    ShareLink     { code: String, link_type: ShareLinkType },   // resolved per account at launch
    ShareLinkV2   { code: String },
    Universe      { universe_id: u64 },                         // resolved to its root place
    FollowUser    { user_id: u64 },
}
enum ParsedInput { Target(LaunchTarget), JobIdOnly(Uuid), Invalid(String), Unsupported(String) }
enum BulkStage { Queued, Resolving, Ticket, Starting, Joining, InGame, Failed(String), Cancelled }
```

## 5. IPC commands

All return `Result<T, AppError>`; `AppError` reaches the UI as `{kind, message}` with kinds such as
`Locked`, `CookieInvalid`, `Challenge` (needs a human in the browser), `RateLimited{retry_at}`,
`BoundSessionRequired`, `RobloxRunning`, `NotFound`, `Network`, `Internal`.

| Area | Command | Returns |
|---|---|---|
| Vault | `vault_status()` | `{mode, locked, accounts}` |
| | `vault_unlock(password)` / `vault_lock()` | — |
| | `vault_set_mode(mode, current_password?, new_password?)` | — |
| | `app_activity()` (throttled, resets the idle timer) | — |
| Accounts | `accounts_list()` | `AccountSummary[]` |
| | `account_add_login()` (opens login window, resolves on capture) | `AccountSummary` |
| | `account_add_cookie(cookie)` (validated first) | `AccountSummary` |
| | `account_update(id, patch)` alias/notes/colour/favourite/groups | `AccountSummary` |
| | `accounts_reorder(ids)` / `accounts_remove(ids)` | — |
| | `accounts_validate(ids?)` | `AccountStatus[]` |
| | `account_reveal_cookie(id, password?)` / `account_copy_cookie(id, password?)` | `String` / — |
| | `account_open_session(id)` (isolated roblox.com window) | — |
| | `groups_list()` / `group_save(input)` / `group_remove(id)` | `Group[]` / `Group` / — |
| | `backup_export(path, password, account_ids?)` / `backup_import(path, password, on_conflict)` | count / `ImportReport` |
| Info | `account_info(id, refresh)` | `AccountInfo` |
| | `presence(ids)` | `Presence[]` |
| | `friends_joinable(account_id)` (Join-user picker) | `FriendPresence[]` |
| Launch | `target_parse(input)` (local, no network) | `ParsedInput` |
| | `launch(account_id, target)` | `Instance` |
| | `bulk_launch({account_ids, target, delay_ms, same_server: off\|pick_first\|follow_first, auto_rejoin})` | `batch_id` |
| | `bulk_cancel(batch_id)` | — |
| | `favourites_list()` / `favourite_save(input)` / `favourite_remove(id)` / `favourites_reorder(ids)` | |
| Instances | `instances_list()` | `Instance[]` |
| | `instance_close(id, force)` / `instances_close_all(force)` | — |
| | `multi_instance_status()` | `{enabled, held, blocking_pid?}` |
| Servers | `game_info(place_id)` | `GameInfo` |
| | `servers_list(place_id, {order, exclude_full, cursor?, account_id?})` | `ServerPage` |
| | `private_servers_list(place_id, account_id, cursor?)` | `ServerPage` |
| Settings | `settings_get()` / `settings_set(patch)` | `Settings` |
| | `data_dir_move(path)` | — |

Events (backend → UI): `accounts://changed`, `instances://changed`, `bulk://progress`, `vault://locked`.
Copy user ID / username and "open profile" stay in the UI (clipboard API, opener plugin).

## 6. Key flows

- **Add via login:** async command opens `login-<uuid>` window on a fresh temp WebView2 profile
  (incognito, no devtools, no autofill, no IPC capability). A worker polls `cookies()` until
  `.ROBLOSECURITY` appears, validates it with `users/v1/users/authenticated`, stores it, closes the window,
  waits for the browser process to exit and deletes the profile. Cookies are injected into session windows
  through WebView2's own cookie manager (Tauri's `set_cookie` drops the leading dot of `.roblox.com`).
  `WEBVIEW2_*` override variables are cleared at start-up so they cannot merge the per-login profiles.
  Profiles are temporary on purpose: the vault stays the only place a cookie is kept. If Roblox ever
  enforces bound sessions, the fallback is a persistent profile per account (a later decision).
- **Launch:** validate → resolve share link / universe (with this account's cookie) → ticket → build URI
  with a fresh `browsertrackerid` and empty channel (production) → start `RobloxPlayerBeta.exe` (path from
  `HKCU\Software\ROBLOX Corporation\Environments\roblox-player\clientExe`) with the URI as its only
  argument → record PID and creation time, close the handle at once (Hyperion has flagged long-held
  handles) → bind the client's `_Player_` log file by that tracker ID → read job ID, user ID and
  disconnects from it. If an update makes Roblox relaunch itself (new PID via the installer), the tracker
  re-binds within 120 s through the log's `userid:`.
- **Same server (bulk), two modes:** *Pick a server first* (default for public games): choose one server
  from the list with room for all N, launch everyone with `RequestGameJob`. *Follow the first account*
  (as in the brief): launch #1 normally, read its job ID from its log (presence as fallback), send the rest
  with `RequestGameJob`. Private-server targets are already one server.
- **Bulk:** strictly one at a time; default stagger 6 s ± 1 s; ticket fetched right before each launch;
  next launch waits for the previous client's log binding; stop the batch on a challenge, an account
  mismatch in the log, a locked vault or multi-instance turned off (the accounts not yet launched are
  cancelled and can be picked again in one click); cancel stops after the current step.
- **Close:** WM_CLOSE twice (the client may show a leave prompt), then TerminateProcess after a few seconds.
- **Auto-rejoin:** a client Roblox disconnects (kicked, server shut down, connection lost: a disconnect
  line in its log whose reason isn't the player's own Leave, 285, or the same account launched elsewhere, 273,
  which a rejoin would kick) is closed after the stagger and its account relaunched into the same target, at
  most 3 tries in a row (a client that stays in a game for 5 minutes starts the count over). A plain exit is not rejoined: closing Roblox's
  window leaves no trace in the log, so it can't be told apart from a crash, and a kicked client stays open
  on its error dialog anyway.

Rate limits: one queue per route, driven by Roblox's `x-ratelimit-limit/remaining/reset` headers (windows
are aligned to the clock minute); 429 waits for `x-ratelimit-reset` (Retry-After is shorter than the real
window); exponential backoff from 1 s; CSRF retry once on any host (tokens rotate within tens of minutes);
challenge 403s (`rblx-challenge-*`) are never retried and tell the user to open the account's session
window. Error bodies come in three shapes (`{errors:[…]}`, a bare string, empty) and are all parsed.

Input validation before anything reaches a URI: place / user IDs are digits (u64), job IDs and access codes
are GUIDs, private-server link codes are digits (25–32 seen), share codes are `[A-Za-z0-9_-]`.

## 7. Releases

- `installer/RoALT.iss`: per-user install (no admin), `{localappdata}\Programs\RoALT`, Windows 11 wizard
  style in dark and light, desktop/start-menu icons, offers Microsoft's WebView2 download if the runtime is missing (the app explains it on start too).
  Uninstall keeps your accounts unless you tick "also delete my data".
- `scripts/build-installer.ps1`: builds the app (`tauri build --no-bundle`, static CRT) and runs ISCC with
  the version from `tauri.conf.json`, output `dist\RoALT-Setup-<version>.exe`.
- `.github/workflows/release.yml` (same pattern as AgentSuite): a `v*` tag builds on windows-latest,
  installs Inno Setup, attaches the setup .exe and `SHA256SUMS.txt` to a **draft** release; you publish it.
  `ci.yml` runs `cargo test`, clippy and `tsc` on every push.
- No code signing yet: SmartScreen will warn on first run. No in-app update check (RoALT only talks to
  Roblox); Settings → About links to the Releases page.

## 8. Milestones

| | Builds | Runnable result |
|---|---|---|
| M1 | scaffold, theme, vault, storage, add via login / paste, validate, groups, backup, installer + CI | add accounts, see them validated, export a backup, install from a setup .exe |
| M2 | info panel, headshots, presence, cache | account detail panel with live data |
| M3 | link parser, ticket, URI, single launch, favourites | launch one account to game / job / private / share link / join user |
| M4 | singleton, process tracking, log watcher, close | several clients side by side, listed and closable |
| M5 | bulk launch, stagger, same server, progress, cancel | launch N accounts into one server |
| M6 | server browser, private servers | pick a server, send selected accounts |
| M7 | auto-rejoin, tray favourites, shortcuts, settings polish | daily-driver build |

Each milestone ends with a summary, test steps, any Roblox behaviour that differed, and a setup .exe.
Tests: URI building (byte-for-byte vs captured website output), link parsing (38 recorded cases), vault
and backup round-trips, response parsing from the fixtures in `docs/research` (no real cookies).

## 9. Needs a real account to confirm (checked at M1–M5 with your accounts)

1. `authentication-ticket` still accepts `{}` when `client-assertion` fails.
2. A `RequestPrivateGame` with only a link code works for an account that never resolved the share link
   (until then, each account resolves its own).
3. Presence with an account's own cookie returns its own job ID.
4. Server-list and `authentication-ticket` rate limits with a cookie (unauthenticated: 3 / min and 1000 / min).
5. A non-owned `ROBLOX_singletonEvent` mutex on client 0.744 (works in other tools as of Oct 2026).
6. Whether launching account B disturbs a running client of account A without the shared-login lock.
