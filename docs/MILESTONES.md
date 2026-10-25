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
