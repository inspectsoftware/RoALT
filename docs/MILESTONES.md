# RoALT milestones

What each milestone built, how to test it, and where Roblox behaved differently from the brief.
Design: [DESIGN.md](DESIGN.md). Every change gets its own setup in `installers/` (gitignored).

## M1 — vault, storage, accounts (v0.1.0)

**Built:** Tauri 2 app with the approved sharp dark/light UI; envelope vault (Windows DPAPI by
default, optional Argon2id + AES-256-GCM master password with auto-lock); SQLite storage; add
accounts through Roblox's own sign-in page in a throw-away WebView2 profile or by pasting a
cookie (validated first); validation on start and on demand; groups, alias, notes, colour,
favourite, drag order, search and filters; reveal/copy behind Windows Hello or the master
password (copy kept out of clipboard history, cleared after 30 s); encrypted `.roalt` backups;
Inno Setup installer, CI, and a tag → draft GitHub release workflow.

**Test:** add an account both ways, validate, reveal/copy, set a master password and lock,
export and re-import a backup. Hand-tested by the user: all worked.

**Roblox differences:** CSRF comes from the real request's 403 (the logout trick ends the
session); `.ROBLOSECURITY` rotates via `Set-Cookie`, so rotated values are stored; hardware-bound
sessions exist but are not enforced for cookie use (staff, 2026-05).

## M2 — account info panel

**Built:** avatars (cached 6 h); presence column (each on-screen account asks about itself,
once a minute, only while visible); info panel with Robux, Plus / Premium (legacy), friend /
follower / following counts, join date and age, email verified, 2-step methods, presence with
game, job ID and "Open game page"; "Open on roblox.com" (signed-in throw-away window; the cookie
it ends with is kept only if it still signs in as the same user); info cache (Settings → Data).

**Test:** select an account: panel fills; Refresh; Open on roblox.com shows the account signed in;
close it; an expired account shows "Cookie expired" instead of a half-empty panel.

**Roblox differences:** Premium no longer sells (Roblox Plus via `user-profile-api`);
`/presence/last-online` is gone; friends-only presence is hidden from other viewers, so each
account checks itself; public endpoints answer anonymously with a dead cookie, so Robux
(cookie-only) is the dead-cookie probe; the friends list endpoint returns empty names.

## M3 — single launch

**Built:** Launch view: paste anything (game link or place ID, `roblox://` link, job ID with
place, classic private server link, `roblox.com/share?code=…` share link incl. v2, universe,
profile link, `@username`) and the type is detected locally; Join someone (friends online, then
your other accounts, each checked with its own cookie); favourites (name + target, encrypted at
rest, sidebar list, rename / reorder / delete, used-last time); account picker (valid cookies
only); Ctrl+Enter. The launch itself works like the website: client-assertion, then
authentication-ticket with cookie + CSRF + Referer, then a `roblox-player:` URI with a fresh
browser tracker id, started through the installed client (or the registered bootstrapper).
A dead or challenged cookie found during launch is recorded on the account.

**Test:** pick an account, paste a game link, Launch: Roblox opens in that game. Paste a job
ID, enter the place ID: joins that server. Paste a private server link or a Server share link:
joins the private server (the account must have access). Join someone → a friend in a game:
joins their server. Save as favourite, click it in the sidebar, Launch again.

**Roblox differences:** the web client sends a `client-assertion` body with the ticket request
(sent here too; an empty body still works); `launchexp:InApp` and no `channel` parameter;
share links resolve per account through `sharelinks/v1/resolve-link` and only Valid invites
join their exact server (Expired ones still open the game, with a warning); launch data over
200 bytes is refused by Roblox, so it is dropped with a warning; usernames resolve through
`users/v1/usernames/users` (banned users excluded).

## M4 — multi-instance and process tracking

**Built:** a multi-instance switch (sidebar and Settings → Launching). When on, RoALT holds a
mutex named exactly like the event Roblox's client creates, so a new client can't tell the old
one to quit. If Roblox is already running unprotected, the switch says so and offers "Close
Roblox". The first time it is turned on, Roblox's stance on multiple clients is shown once.
While RoALT's clients run (and while one is launching), Roblox's shared sign-in files are held
locked so clients can't overwrite each other's session; they are released when the last client
ends. The Launch view lists the clients RoALT started: account, state (starting, joining, in
game, disconnected, ended, with problems such as a human check or a wrong account shown), job
ID and uptime; close one (graceful, then forced after 5 s), close all, clear ended. Clients are
tied to accounts through their own log file (by the launch's browser tracker id), followed
through Roblox's updater relaunch and through bootstrappers such as Bloxstrap.

**Test:** turn multi-instance on with Roblox closed; launch two accounts: both clients stay
open, each row shows its account in game. Close one from its row. Turn protection off and on in
Settings. Start Roblox normally first, then turn multi-instance on: the sidebar explains and
"Close Roblox" fixes it.

**Roblox differences:** the lock is now `ROBLOX_singletonEvent` (an event in client 0.744), not
the old `ROBLOX_singletonMutex`; a mutex with that exact name blocks it. Roblox's tray process can
already hold the name, so it counts as running. Logs are the only reliable source of the job id
and of which account a client signed in as; presence lags.

## M5 — bulk launch and same server

**Built:** the Launch view picks several accounts (checkboxes). Two or more launch as a batch:
strictly one after another, each waiting until the previous client's log is bound, then the
stagger (Settings → Launching, default 6 s, ±1 s). "Same server" puts everyone in one server:
"Pick a server" chooses one with room for all of them from the server list (Recommended for the
first account, else the fullest that fits) before anyone starts; "Follow the first" reads the
first client's server from its log (its own presence as a fallback, only when it shows that
game). The Batch panel shows each account's steps (ticket, run, join, in game), the count in
game, a Cancel (after the current step), and why a batch stopped: a human check, a wrong account
in a client, a locked vault or multi-instance turned off stop it; one dead cookie fails only its
account. After a stop, one click picks the accounts that didn't launch. Single launches wait
while a batch runs.

**Test:** turn on multi-instance, pick three accounts, paste a game link, turn on Same server,
Launch 3 accounts: all three end up in the same job ID. Cancel a batch midway: the rest show
cancelled. Try Follow the first.

**Roblox differences:** the server list (v2, `orderBy`) is the website's; Recommended and Best
ping need a signed-in account, and a guest gets 3 list calls a minute. Picking the server before
launching avoids detecting it afterwards; presence alone is too slow to follow reliably.

## M6 — server browser

**Built:** a Servers view. Paste a game link, place ID, game ID link or share link (share links
open as the first picked account); the header shows the game's icon, name, creator, place and
universe IDs and how many play. Public servers: Most full, Most empty, Recommended and Best
ping (the last two only with an account picked), Hide full, 25 per page with Load next
(duplicates across pages removed), Refresh, copy a job ID. Private servers the first picked
account can see (joinable ones carry an access code that is never shown). Pick a server, choose
the accounts in the bottom bar, Join: one account launches directly, several as a batch. It warns
when the server lacks room and refuses accounts Roblox restricts.

**Test:** load a popular game, sort and page through servers, pick one and join with two
accounts: both land in that job ID. Open the Private tab with an account that owns or was
invited to a private server.

**Roblox differences:** the guest flag for selling private servers is false for games that
still have them, so the Private tab is always offered and the account's own list decides; game
info, icons and the place → universe lookup work without a cookie; icons are only accepted from
https *.rbxcdn.com.
