# RoALT

A local-only Roblox account manager for Windows, for the accounts you own: keep them signed in,
check they still work, and launch any of them into a game.

## Install

Download `RoALT-Setup-<version>.exe` from [Releases](https://github.com/inspectsoftware/RoALT/releases)
and run it. No admin rights needed; it installs to `%LOCALAPPDATA%\Programs\RoALT`.

- Windows 10 1809 or later, or Windows 11, x64, with the Microsoft Edge WebView2 Runtime (Setup offers
  Microsoft's download when it is missing).
- The build is not code-signed yet: SmartScreen warns on first run. Choose **More info → Run anyway**,
  after checking the SHA-256 against the release's `SHA256SUMS.txt`.

Your accounts and settings live in `%LOCALAPPDATA%\RoALT`. Uninstalling keeps them unless you choose
to delete them.

## Build from source

Needs Node.js 24, Rust (stable, with the Visual Studio Build Tools' C++ workload) and, for the
installer, [Inno Setup 6.6 or newer](https://jrsoftware.org/isdl.php).

```powershell
npm ci
npm run tauri dev                                                   # run it
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1  # dist-installer\RoALT-Setup-<version>.exe
```

## Cut a release

1. Set the same version in `src-tauri/tauri.conf.json`, `package.json` and `src-tauri/Cargo.toml`,
   then run `cargo update -p roalt --offline` in `src-tauri` so `Cargo.lock` matches (CI builds `--locked`).
2. Commit all four files, then tag and push the tag: `git tag vX.Y.Z && git push origin vX.Y.Z`.
3. The Release workflow builds the installer and puts it with `SHA256SUMS.txt` on a draft release.
   Check it, then publish the draft.

## Privacy

RoALT talks only to `*.roblox.com` and `*.rbxcdn.com`. No telemetry, no update server, no account of
ours: everything stays on your PC, and cookies are encrypted with Windows DPAPI (or your master password).
