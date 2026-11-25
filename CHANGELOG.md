# Changelog

What changed in each RoALT release. Patch releases between these list their commits on
[GitHub Releases](https://github.com/inspectsoftware/RoALT/releases).

## 1.4.0 (unreleased)

### New

- **RoALT Remote (off by default).** Watch and manage your clients from your phone or another PC
  at [remote.roalt.dev](https://remote.roalt.dev/). Turn it on in Settings › Remote, click Pair
  device, and scan the QR code or type the 8-character code. Nothing connects until you do.
- **You approve every device on the PC.** A code pairing also shows a 6-digit code on both
  screens: approve only when they match. Every new pairing shows a notice, a second device trying
  the same code stops the pairing, and a PC takes up to 64 devices.
- **New devices can only watch.** View shows your clients, live previews, the monitor and crashes.
  Give a device Control in Settings › Remote and it can also launch accounts and favourites, close
  and arrange clients, change limits, stop every client, schedule a shutdown, and tap, steer and
  press keys inside a client. Control can spend Robux and trade items in game, so give it only to
  your own devices, and never show the QR code or the pairing code on a stream or to a "helper".
- **End-to-end encrypted.** What the PC and the device send each other is encrypted on one and
  opened only on the other. Cookies, vault passwords, recovery codes, account notes and launch
  arguments never leave the PC.
- **The relay** at `wss://remote.roalt.dev/ws/remote` only passes the encrypted traffic on. It sees
  IP addresses, the PC's RoALT version, when the PC is online, how many devices connect and how much
  they send. See
  [Safety and privacy](https://github.com/inspectsoftware/RoALT#-safety-and-privacy) and the
  [Remote threat model](https://github.com/inspectsoftware/RoALT/blob/main/docs/REMOTE_THREAT_MODEL.md).
- **Always in view.** A title-bar pill shows which devices are connected, and Revoke removes a
  device for good. Remote can pause itself on Public networks.
- **No stuck input.** Held keys, the camera drag and touches are let go when the device goes quiet,
  Windows locks, or the device loses Control. A RoALT too old for the relay is told to update.
- **Hall of fame.** The app menu thanks the contributors and beta testers who helped build and
  test RoALT, with their Discord usernames one click from your clipboard.

### Fixed

- A client no longer stays on "Starting" while its game runs. Roblox can keep an idle copy of
  itself in the tray and hand the next launch to it; RoALT now closes that idle copy before it
  launches. Copies that are in a game are left alone.

## 1.3.0 (2026-11-20)

### New

- **First-run setup.** New users get a short setup: language and look, how the vault is unlocked
  (Windows sign-in or a master password), what RoALT is for (several accounts at once, keeping them
  online), and the first account. A quick tour of Home, Play, Accounts and Settings follows.
  Settings › About › Setup and tour runs it again.
- **Take control.** Tick it in a client's menu on Home and RoALT leaves that client alone until it
  closes: no auto-rejoin, restarts, anti-AFK, CPU limits or window layout. The window is restored,
  unmuted and brought to the front. Untick it to hand the client back.
- **Lighter previews.** Home's previews can be turned off or run at 1 FPS. Capturing no longer
  waits on the GPU, all previews together are capped, and they slow down while RoALT isn't in
  front. On Windows 10 previews only redraw when something changes. For a busy PC, the advice
  suggests Off.
- **Quit question.** Closing RoALT while it looks after clients now asks whether to keep it running
  in the tray, since quitting stops auto-rejoin, restarts and anti-AFK.
- **Lock when Windows locks.** Win+L, the lock screen or sleep locks a master-password vault too
  (Settings › Security). It stays open while clients run that rely on auto-rejoin, restarts or
  anti-AFK. If anti-AFK can't press because Windows is locked, Discord is told.
- **News switch.** News on Home can be turned off in Settings › About; RoALT then asks GitHub, the
  DevForum and Roblox's newsroom nothing.
- **Discord.** Choose when the ping fires: urgent alerts only, or warnings too. Rich Presence can
  show or hide the game, the account count, the elapsed time and the buttons, with your own two
  lines of text (`{game}`, `{count}`).
- **Limits for all clients.** A client's limits dialog can apply its limits to every running
  client at once.
- **Group defaults** can also clear the overrides of the group's accounts, so the group's values
  apply to them.
- **More Roblox share links** fill in Play: friend game invites, shared servers, shared private
  servers, game links and player profiles.
- **Hotkeys** are set by pressing the keys, with a button to clear one.

### Changed

- **Removed:** Clips, Trades, Outfits, owning or buying private servers, and the Friend requests
  view. Joining private servers by link, the Servers page's Private tab, "Make friends" between
  your own accounts, Limiteds and Browse's purchase lock stay.
- The sign-in window now only allows Roblox, Google and Apple sign-in pages, and Browse only
  roblox.com. The sign-in uses its own browser profile, which is wiped afterwards.
- Exported cookie files are deleted when RoALT quits (and still after 10 minutes), and their folder
  is readable only by you.
- Applying a performance profile to all accounts also clears groups' FPS caps that are higher
  than the profile's.
- Clearer error messages when Roblox refuses or doesn't answer a request.
- The Add account dialog leads with Roblox's own sign-in; cookie, bulk and import are one click
  further.

### Fixed

- A client waiting to be restarted after a crash is no longer forgotten when you switch language.
- Two settings changed at the same moment no longer undo one another.
- Restoring a backup's settings can no longer freeze RoALT's window.
- Anti-AFK's help texts now describe what it does: an F13 press while you're away, and a minimized
  client is restored for it and minimized again.
