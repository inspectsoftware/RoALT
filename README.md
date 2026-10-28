<div align="center">

<img src="src-tauri/icons/app-icon.svg" width="88" alt="RoALT logo">

# RoALT

**A fast, local-only Roblox account manager for Windows.**
Keep your accounts signed in, check they still work, and launch any of them — or all of them — into a game.

[![Latest release](https://img.shields.io/github/v/release/inspectsoftware/RoALT?style=flat-square&color=b4f000&label=release)](https://github.com/inspectsoftware/RoALT/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/inspectsoftware/RoALT/total?style=flat-square&color=2b2b2b)](https://github.com/inspectsoftware/RoALT/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/inspectsoftware/RoALT/ci.yml?branch=main&style=flat-square&label=CI)](https://github.com/inspectsoftware/RoALT/actions/workflows/ci.yml)
![Windows 10/11](https://img.shields.io/badge/Windows-10%20%7C%2011-0078d4?style=flat-square&logo=windows)
[![Discord](https://img.shields.io/badge/Discord-join%20the%20server-5865f2?style=flat-square&logo=discord&logoColor=white)](https://discord.gg/Q2vrgDKb3)

[**Download**](https://github.com/inspectsoftware/RoALT/releases/latest) ·
[**Discord**](https://discord.gg/Q2vrgDKb3) ·
[Build from source](#build-from-source) ·
[Privacy](#privacy)

<img src="docs/images/accounts.png" alt="RoALT's Accounts view: a list of accounts with their status, groups and a details panel" width="900">

</div>

## Features

| | |
|---|---|
| 🔐 **Encrypted vault** | Cookies are encrypted with Windows DPAPI, or with your own master password (Argon2id + AES-256-GCM) and auto-lock. Reveal and copy sit behind Windows Hello or the master password. |
| 👥 **Accounts at a glance** | Avatars, live presence, Robux, Plus, friends, followers, 2-step status. Groups, aliases, notes, favourites, search and filters. Validate one or all in a click. |
| 🚀 **Launch anything** | Paste a game link, place ID, job ID, private server or share link, a profile or `@username` — RoALT works out what it is. Join a friend, or save a target as a favourite. |
| 🪟 **Multi-instance** | Run several clients side by side. Launch a batch one after another, all into **the same server** if you like. |
| 🔁 **Auto-rejoin and restart** | A client that gets disconnected rejoins the same game on its own. Opt-in: crashed, frozen or memory-hungry clients restart too. A crash breaker gives up on an account after too many tries; accounts can be allowed to stay disconnected. Crash history with the log's reason. |
| 🛑 **Kill all** | Hold one button (or use the tray) to end every Roblox process and stop pending launches and rejoins. |
| ⚙️ **Launch control** | Custom launch arguments (default and per account), a cooldown between launches, a configurable close grace. |
| 📈 **Monitor** | CPU, memory, disk/network and uptime per client, with sparklines and totals. |
| 💤 **Anti-AFK** | Off by default. While you're away from the PC, every few minutes each client is briefly brought to the front and gets one Space press, so Roblox doesn't kick it after 20 idle minutes. Per-client opt-out. |
| 🔔 **Discord alerts** | Crashes, disconnects, restarts and finished batches posted to your own webhook (kept encrypted in the vault). Opt-in. |
| 🎚️ **Per-client limits** | Cap one client's CPU (down to 5%), put it in efficiency mode, pin it to cores or trim its memory, so background accounts don't slow your PC. An adaptive mode limits every client that isn't in front. |
| 📶 **Bandwidth limit** | Opt-in cap on all Roblox clients' upload, through a Windows QoS policy. |
| ⬆️ **Roblox updates** | Shows when Roblox is out of date and installs the update from Roblox's own installer, checked for Roblox's signature first. Optional auto-update when no client runs. |
| 🗺️ **Server browser** | Browse public servers by players or ping, see private servers, and send any account into the one you pick. |
| 🧰 **Quality of life** | Tray menu with favourites, encrypted backups, relaunch an ended client in one click, keyboard shortcuts, dark and light themes. |

## Install

1. Download **`RoALT-Setup-<version>.exe`** from the [latest release](https://github.com/inspectsoftware/RoALT/releases/latest).
2. Run it. No admin rights needed; it installs to `%LOCALAPPDATA%\Programs\RoALT`.

**Needs:** Windows 10 1809 or later, or Windows 11 (x64), with the Microsoft Edge WebView2 Runtime. Setup offers to download it when it's missing.

> [!NOTE]
> The installer isn't code-signed yet, so SmartScreen warns on first run. Check the file's SHA-256 against
> `SHA256SUMS.txt` on the release, then choose **More info → Run anyway**.

Your accounts and settings live in `%LOCALAPPDATA%\RoALT`. Uninstalling keeps them unless you choose to delete them.

## Privacy

- RoALT talks **only** to `*.roblox.com` and `*.rbxcdn.com`. The one exception is opt-in: if you paste a
  Discord webhook in the Discord tab, alerts go to that webhook on `discord.com`. Nothing goes there until you
  do; the webhook URL is kept encrypted in the vault, and messages name accounts by display name and username only.
- Roblox updates come from Roblox only: the version from `clientsettingscdn.roblox.com`, the installer from
  `setup.rbxcdn.com`, run only when its signature is Roblox Corporation's.
- No telemetry, no update server for RoALT, no account with us. Everything stays on your PC.
- RoALT never reads or modifies the Roblox client. Limits and multi-instance work through Windows itself.
  The optional anti-AFK briefly brings a client to the front and presses Space the way a keyboard does.
- The optional bandwidth limit adds a Windows QoS policy (`RoALT-Roblox`, cleared when Windows restarts) through an admin prompt, only when you click Apply.
  It is the only change RoALT makes to Windows itself.

## Community

Questions, ideas or bug reports? Join the **[Discord server](https://discord.gg/Q2vrgDKb3)** or
[open an issue](https://github.com/inspectsoftware/RoALT/issues).

## Build from source

Needs Node.js 24, Rust (stable, with the Visual Studio Build Tools' C++ workload) and, for the
installer, [Inno Setup 6.6 or newer](https://jrsoftware.org/isdl.php).

```powershell
npm ci
npm run tauri dev                                                          # run it
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1        # dist-installer\RoALT-Setup-<version>.exe
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1 -Dev   # also keeps installers\RoALT-Setup-<version>-dev-<stamp>.exe
```

`npm run dev` alone runs the UI in a browser against a built-in mock of the backend.
How it works is in [docs/DESIGN.md](docs/DESIGN.md); what each milestone added is in [docs/MILESTONES.md](docs/MILESTONES.md).

<details>
<summary><b>Cut a release</b></summary>

1. Set the same version in `src-tauri/tauri.conf.json`, `package.json` and `src-tauri/Cargo.toml`,
   then run `cargo update -p roalt --offline` in `src-tauri` so `Cargo.lock` matches (CI builds `--locked`).
2. Commit all four files, then tag and push the tag: `git tag vX.Y.Z && git push origin vX.Y.Z`.
3. The Release workflow builds the installer and puts it with `SHA256SUMS.txt` on a draft release.
   Check it, then publish the draft.

</details>

## Disclaimer

RoALT is an independent project, not affiliated with or endorsed by Roblox Corporation. Use it only
with accounts you own. Roblox doesn't officially support running several clients at once and may
change how its client behaves at any time.
