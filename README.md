# AltDock

A local-only Roblox account manager and multi-client launcher for Windows, with **background
sessions**: Roblox clients can run in separate Windows sessions, so anti-AFK and macros never take
your mouse, keyboard or focus.

AltDock is a fork of [RoALT](https://github.com/inspectsoftware/RoALT) by inspectsoftware, used under
the [MIT License](LICENSE). It is **not RoALT**, and it is not affiliated with, endorsed by or supported
by RoALT, inspectsoftware or Roblox Corporation. Please don't ask RoALT for help with AltDock.

## Features

- Everything RoALT has: accounts kept signed in and encrypted on your PC, launching one or many
  accounts into any game or server, several clients side by side, auto-rejoin, live previews,
  performance controls, server browser, private servers, Discord webhook alerts.
- **Background sessions** (in progress): clients run in their own Windows sessions on this PC, out of
  your way.
- **AutoHotkey macros per account** (in progress): assign a macro to an account; it starts when that
  account's client is in a game.
- **Anti-AFK per client** (coming): its own key and interval for each client.

## Privacy

AltDock talks to Roblox (and Roblox's DevForum and newsroom for the News tab), and to Discord only
once you set a webhook. It sends nothing to RoALT: no online counter, no update checks, no RoALT
release feed. Its data lives in `%LOCALAPPDATA%\AltDock`, apart from any RoALT install.

## Build from source

Needs Windows, Rust, Node.js and (for the encrypted database) Strawberry Perl.

```
npm ci
npm run tauri dev        # run
npm run build            # UI type check and build
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1   # installer (needs Inno Setup 6.6+)
```

## License

MIT, see [LICENSE](LICENSE) (RoALT's copyright notice is kept there) and [NOTICE.md](NOTICE.md).
