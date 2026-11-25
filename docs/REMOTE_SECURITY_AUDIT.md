# RoALT Remote: release-readiness verdict

## 1. Verdict

**Ship after fixes. Do not ship the current build.** Four problems break promises the product makes:

- **The relay can read manual-code pairing.** The browser sends its identity key and deviceId in plaintext, and nothing binds them to the SAS. The SAS also has no commit-then-reveal step. A compromised relay can therefore swap in its own key, or grind a matching 6-digit SAS in seconds, and end up as a Control device.
- **A stranger can do the same without compromising the relay.** They need only the 8-character code (from a stream or screen share), because the relay lets any self-made "PC" overwrite another PC's code or pairId.
- **QR pairing grants Control with no click on the desktop.** Anyone who sees the QR within its 120 s window gets in.
- **One script can take the relay down for everyone.** It can be OOM-killed (`mem_limit: 128m` with message-count backpressure), or starved with idle sockets, because there is no idle timeout and no connection cap.

**Blocking:**

- **F1 + F2 + F14:** Bind the device identity inside the sealed channel. Add a commitment round to the SAS. Lock the pending request and have approve check the SAS.
- **F3:** The relay must reject a pair_open for a pairId or code another PC owns.
- **F4:** QR joins go through Approve, and new devices default to View.
- **F5 + F6:** Byte-based queue budgets, a viewer cap per PC, registration and idle timeouts, connection caps per IP and globally, and nofile/mem sizing.

**Must also land before the first desktop release, because shipped binaries freeze them:**

- **F8:** A separate dashboard origin, since `WEB_BASE_URL` is hardcoded at mod.rs:43.
- **F21:** A protocol/app version field in pc_hello.
- **F22:** README privacy and host-list corrections.
- **F23:** CI coverage plus `web/**` in paths-ignore.

**Strongly recommended for the same release** (cheap, user-visible safety): F7 (snapshot size), F9 (snapshot redaction), F10 (stuck input), F11 (durable Disconnect all), F12 (cross-PC rpcResult).

I re-checked three points myself:
- The QR path hardcodes `RemoteRole::Control` at mod.rs:1101.
- PairOpen inserts into `pair_rooms`/`manual_codes` with no ownership check at lib.rs:576-578.
- docker-compose.yml:11 has `mem_limit: 128m`.

## 2. Findings ranked by final severity

| # | Title | Severity | File:line | One-line fix |
|---|---|---|---|---|
| F1 | Manual pairing: browser `devicePubB64`/`deviceId` travel in plaintext and are not bound to the SAS, so the relay swaps in its own key and the SAS still matches | critical | src-tauri/src/remote/mod.rs:1100-1130; web/remote/src/transport.ts:532-541 | Send `{deviceId, devicePub, label}` sealed under pairKey, signed by the identity key over pairId and both ephemerals, and enroll only the decrypted values |
| F2 | Manual SAS has no commitment: a MITM (relay, or a stranger via F3) grinds about 1e6 ephemerals in seconds to match the 6 digits and gets a full MITM plus a persistent Control device | critical | src-tauri/src/remote/crypto.rs:311-326; web/remote/src/crypto.ts:201-214 | Browser sends `H(ephem‖devicePub‖nonce)` first and reveals after the PC's ephemeral. Derive the SAS over the full transcript (both ephems, devicePub, deviceId, pcStaticPub) |
| F3 | Any self-registered "PC" can overwrite another PC's pairing room or manual-code mapping, so the victim's phone lands in the attacker's room (PoC confirmed). `remove_pair_room` also deletes code mappings it does not own | high | server/remote-relay/src/lib.rs:569-579, 763-767 | `entry()` check-and-insert, reject `taken` if the pairId or code belongs to another pc_id, and use `remove_if` on cleanup. Better, have the relay generate the codes |
| F4 | QR pairing completes instantly as Control with no desktop approval, no role choice and no toast. The dashboard also auto-pairs any `#pair` link without confirmation | high (raised from medium) | src-tauri/src/remote/mod.rs:1086-1101; web/remote/src/components/PairView.tsx:29-47; src/features/remote/RemotePanel.tsx:50-55 | Route QR joins through pending → Approve (no SAS needed), default to View, toast every new pairing, and add copy warning against showing the QR or code; confirm "Pair with <pcName>?" in the dashboard |
| F5 | Relay OOM: a 64-message queue × 256 KiB frames is about 14 MiB per stalled viewer, there is no viewer cap per PC, and anyone can self-pair. About 10 sockets exceed 128m and every user is dropped (test confirmed). Slow phones at scale reach the same OOM | high | server/remote-relay/src/lib.rs:36-38, 330, 383-385, 402-404; docker-compose.yml:11 | Byte budget per socket (1-2 MiB) with drop or close on sustained stall, a global queued-bytes budget, viewers per PC ≤16, a smaller viewer→PC frame cap, and mem_limit sized to these caps |
| F6 | No registration or idle timeout, no relay pings, no connection cap per IP or globally. Idle sockets exhaust FDs or memory. Dead phones stay attached for about 15 min. Cleanup and pc_hello scan all rooms and viewers | high | server/remote-relay/src/lib.rs:343, 144-162, 471-480, 785-789 | 10 s registration timeout, about 70 s read-idle timeout using the existing 25 s pings, a global semaphore, caps per IPv4/32 and IPv6/64, `ulimits: nofile`, indexes instead of scans |
| F7 | A snapshot over 256 KiB (hundreds of accounts, long notes) makes the relay drop the PC socket, which then reconnects in a loop and leaves Remote unusable for heavy users. Preview JPEG size is also unchecked | medium | src-tauri/src/remote/mod.rs:734-741, 786-790, 899-900, 915-921; server/remote-relay/src/lib.rs:159-160, 362 | Redacted DTOs (F9), size check before sealing, paged account events, and an explicit relay error instead of a socket drop |
| F8 | The dashboard shares the `https://roalt.dev` origin with the marketing site and the proxied Umami `/u/script.js`. Any same-origin script can use the non-extractable key and room tokens to drive every paired PC. The origin is hardcoded in the desktop | medium (release-timing blocker) | web/remote/src/storage.ts:4, 46-74; src-tauri/src/remote/mod.rs:43 | Move to a dedicated origin (e.g. remote.roalt.dev) with a dashboard-only strict CSP before 1.3.x ships, and update WEB_BASE_URL and the relay default in the same change |
| F9 | Snapshot sends every device (View included) account notes, launch args, custom flags, profiles, and private-server link/access codes (favourites, plus instances and bulk regardless of vault lock). The dashboard reads none of these fields | medium | src-tauri/src/remote/dispatch.rs:21-57; mod.rs:916-924 | Remote-only DTOs with only the rendered fields, targets redacted to kind+placeId, favourites by id (PC resolves the target), plus a serialization test asserting the secrets are absent |
| F10 | Held keys, RMB drag and touch contacts stay pressed when the phone goes quiet (no deadman, which the spec §5.3 requires). Release events are refused while locked or after a downgrade to View. Capture and encode keep running for dead viewers | medium | src-tauri/src/instances/remote_input.rs:110-133; src-tauri/src/remote/dispatch.rs:242-247; mod.rs:688-696, 894-897, 964-973 | Worker deadman (about 750 ms) with the dashboard re-sending held state, release events always allowed, release_all on lock or role change, and an E2EE heartbeat with a session timeout of about 15 s |
| F11 | "Disconnect all" pill is not durable: the kicked viewer keeps its subscription and valid token, and re-attaches by resubscribing or on the next pc_hello | medium | src-tauri/src/remote/mod.rs:905-914; server/remote-relay/src/lib.rs:616-630, 471-475, 713-724 | Make it a real pause (refuse viewer_joined or turn Remote off until resumed), offer Revoke next to it, and fix the tooltip |
| F12 | A hostile paired PC can resolve or reject another PC's pending RPCs (global sequential reqId, no pcId check) and inject a forged `targetParse` that redirects a bulk launch | medium | web/remote/src/transport.ts:351-358, 56, 69-70, 436 | Key pending by `pcId:reqId` and accept only from the matching PC, use random reqIds, and validate the targetParse shape and show its details |
| F13 | Relay is an unmetered tunnel: no byte or message rate limits, and invalid or unregistered binary frames are silently ignored without closing | medium | server/remote-relay/src/lib.rs:345-407 | Token bucket per connection (separate PC and viewer budgets), and close on binary while unregistered or to unattached targets |
| F14 | Manual-pair `pending` is silently overwritten by the latest joiner. Approve does not check the SAS the user saw. The attacker-controlled label is shown inside the SAS sentence. Control is preselected | low (fixed together with F1/F2) | src-tauri/src/remote/mod.rs:1120-1131, 334-350; src/features/remote/PairDialog.tsx:39, 74-78 | Lock to the first pending (abort on a second join), pass the displayed SAS or a nonce to approve, strip bidi/Cf characters from labels, default to View |
| F15 | Pairing rate limit is keyed on the exact IP. IPv6 /64 rotation and drive-by browsers (no Origin check) bypass it, and there is no global not_found budget | low | server/remote-relay/src/lib.rs:43-45, 112-125, 148-158 | Bucket by IPv6 /64, global not_found budget, close after rate_limited, Origin check |
| F16 | Pairing blobs use the same key and AAD in both directions (join can be reflected as ack), and ack fields are stored unvalidated. One bad ack breaks the subscribe for all PCs, or overwrites another PC's record by pcId | low | web/remote/src/transport.ts:476-487, 535-544, 629-638; web/remote/src/storage.ts:70-75 | Direction-specific AAD, strict ack validation, refuse a pcId overwrite with a different pcPub, relay parses subscribe items individually |
| F17 | Handshake has no freshness or key confirmation: replayed handshakes spoof "connected" on the desktop badge and start encrypted streaming. The browser accepts a stale PC handshake | low | src-tauri/src/remote/mod.rs:720-741; web/remote/src/crypto.ts:77-79; transport.ts:273-279 | SIGMA-style signature over both ephemerals, or mark the session connected only after the first valid data frame |
| F18 | Revoked devices can still learn when the PC is online (pc_revoked vs pc_offline, plus a push on pc_hello) | low | server/remote-relay/src/lib.rs:476-478, 500-502, 725-731 | Same reply for invalid token and offline, no pushes to unauthorized subscribers |
| F19 | Relay URL is a bearer target: pc_hello sends a static `pcSecretHash` to any configured relay, and backup restore can silently change relayUrl. A malicious relay can replay it to squat the PC on the official relay | low | src-tauri/src/remote/mod.rs:180-196, 619-625; commands.rs:3570, 3607-3610 | Exclude the remote section from backup and restore, confirm relay changes, per-relay derived secret or challenge-response |
| F20 | Decrypted plaintext (up to 200 B) of unparseable requests is logged. Version skew makes this common, and requests can carry link codes | low | src-tauri/src/remote/mod.rs:768 | Log only the error kind, op and length |
| F21 | No protocol or app version in pc_hello or subscribe, and unknown binary versions are silently dropped. Old or vulnerable builds can never be gated | low (pre-release must-do) | src-tauri/src/remote/mod.rs:619-624; server/remote-relay/src/lib.rs:205-223, 362 | Add `proto` and `app` now, add a relay `MIN_APP_VERSION` → `upgrade_required`, explicit error on version mismatch |
| F22 | README and PRODUCT privacy claims become false ("only RoALT server is an online counter", exhaustive host list without the relay). Relay metadata exposure is not documented | low (pre-release must-do) | README.md:62, 422, 437 | Document wss://roalt.dev/ws/remote and what the relay sees, and state the log retention for /ws/remote |
| F23 | Release pipeline: a push to main auto-publishes, `web/**` is not ignored, and neither the relay tests nor the Rust↔WebCrypto interop tests run in CI or release gates. Remote is mixed with Hall of Fame in the tree, and the CHANGELOG has no Remote section | low (pre-release must-do) | .github/workflows/release.yml:3-24, 107, 110 | Add `web/**` to paths-ignore, add a CI job for web/remote test+build and relay `cargo test`, split the commits, write the changelog |
| F24 | Synchronized reconnect storm after a relay restart: no jitter on the desktop, fixed 2.5 s on the web | low | src-tauri/src/remote/mod.rs:502, 567-577; web/remote/src/transport.ts:204 | Full-jitter exponential backoff on both, pause while `document.hidden` |
| F25 | Relay connect has no timeout and the socket no read deadline: a hang blocks Disable, and half-open sockets look Connected | low | src-tauri/src/remote/mod.rs:615, 936-937 | Timeout connect (about 15 s), race it with ctrl_rx, require an inbound frame within about 60 s |
| F26 | No rate or concurrency limits per device on RPCs, snapshots or sessions on the PC. The unbounded input channel queues ReleaseAll behind stale taps | low | src-tauri/src/remote/mod.rs:786-821; remote_input.rs:105 | Token bucket per session, ≤2-3 sessions per device, bounded input channel with a generation-counter release |
| F27 | Preview encoding is not shared across viewers and blocks the session select loop (input and revoke wait behind the encode) | low | src-tauri/src/remote/mod.rs:1021-1060 | Cache JPEGs per (instance, seq, width), encode off-loop, preview budget per PC |
| F28 | Preview width clamp not enforced: remote and desktop share one capture and picture per HWND, so mip levels thrash full-size textures and the phone gets 960-wide JPEGs | low | src-tauri/src/instances/capture.rs:139, 178, 303, 407-461; src-tauri/src/remote/preview.rs:100, 116 | Key the scaled textures and picture by (hwnd, level), reject or downscale shots wider than clamped_w, cap the JPEG size |
| F29 | Injected input is not confined to the client: whole-window bounds, no hit test, and moves or right-button-up ignore focus changes | low | src-tauri/src/instances/remote_input.rs:186-234, 307-331 | Client rect mapping, `GetAncestor(WindowFromPoint)==hwnd` check on Down, release on focus loss |
| F30 | No "watching" indicator: Win11 capture is borderless, and the only cue is a title-bar pill that looks the same for idle and streaming. View devices can start previews freely | low | src-tauri/src/instances/capture.rs:248-256, 385; src/features/remote/RemoteBadge.tsx:12 | Distinct watching state in the tray and pill with one-click stop, plus a setting to refuse previews for View devices |
| F31 | A hostile PC can starve other PCs on the shared viewer queue and inbound chain, and grow preview state without limit with fresh instanceIds | low | server/remote-relay/src/lib.rs:383-385; web/remote/src/transport.ts:184-188, 379-397 | Budget per (viewer, PC), separate inbound chains per PC, drop unsolicited previews, cap preview maps |
| F32 | A malformed snapshot from any paired PC blanks the whole dashboard (no error boundary) and keeps doing so on reload, so the user cannot reach "Forget PC" | low | web/remote/src/App.tsx:75-83, 129; transport.ts:318-340 | Validate events, error boundary per PC, defensive aggregates |
| F33 | Launch and Clients view state survives a PC switch: a link code and account ids pasted for PC A are sent to PC B | low | web/remote/src/App.tsx:260-262; LaunchView.tsx:16-18, 70-76 | Add `key={activeRt.pc.pcId}` to the views and drop stale targetParse results |
| F34 | More than 64 paired devices: the oldest are silently treated as revoked by the relay, and device rows grow without bound | low | src-tauri/src/remote/mod.rs:591; server/remote-relay/src/lib.rs:451-454, 511-514 | Refuse pairing beyond 64 with a message, or prune stale devices |
| F35 | Relay's 16-subscription cap is silent: PC 17 and later stay at "Connecting…" forever | low | server/remote-relay/src/lib.rs:30, 693 | Return `pc_err too_many` and surface it in the UI, or open an extra socket |
| F36 | Docker hardening: no cap_drop, pids_limit, cpus or nofile. Images unpinned, alpine:3.21 is past end of support. X-Real-IP is trusted from any peer | low | server/remote-relay/docker-compose.yml:11; Dockerfile:1, 8 | `cap_drop: [ALL]`, limits, `FROM scratch`/distroless, digest pins |
| F37 | Custom relay setting cannot work against the hosted dashboard (CSP connect-src), and the README proxy configs differ from production. Widening the CSP would arm phishing `?relay=` links | low | web/remote/src/storage.ts:86-89; src-tauri/src/remote/mod.rs:291-297; README.md:39, 58-70 | Remove `?relay=` from the hosted dashboard (or confirm the host first), never widen connect-src, publish the real Caddy config |
| F38 | One deviceId per browser is sent to the relay for every PC, so the relay can link a phone to all the PCs it controls | info | web/remote/src/transport.ts:206; storage.ts:47-60 | deviceId (and key) per PC |
| F39 | Role parsing fails open to Control (anything other than "readonly"), and column, approve and QR defaults are Control | info | src-tauri/src/storage/mod.rs:1041, 1063; 020_remote.sql:7; commands.rs:4193 | Parse "control" explicitly, everything else → Readonly, defaults readonly |
| F40 | A paired PC can rename itself live to match another PC in the switcher | info | web/remote/src/App.tsx:151, 251 | Show the name stored at pairing as primary, flag renames and duplicates |
| F41 | PID reuse window: a preview can capture a non-Roblox window just after a client exits | info | src-tauri/src/instances/tracker.rs:888-893; window.rs:111 | Check the PID start time or exe before capture |
| F42 | Dev-only advisories in vitest 3.2.x (tinypool, @vitest/mocker). The shipped bundle is clean (`--omit=dev` reports 0) | info | web/remote/package.json:23-25 | Bump vitest to a version that fixes them and is compatible with vite 8 |

## 3. Merged duplicates

- **F1** merges: "Manual-code pairing never authenticates the browser's identity key" (critical) and the variant-1 half of "Manual-code pairing SAS does not bind the browser's identity key…" (high, transport.ts:532).
- **F2** merges: "Manual-code SAS has no commitment… grind" (critical), "6-digit SAS has no commitment…" (high), and the variant-2 half of the transport.ts:532 finding. The final severity is critical, because F3 makes this reachable by a stranger who saw the code, with no relay compromise.
- **F3** merges: "Relay lets any client that says it is a PC overwrite another PC's pairing room" (high) and "Any PC can take over another PC's manual code or pairId" (medium, which adds the `remove_pair_room` mapping bug).
- **F4** merges five QR findings (mod.rs:1092/1093 ×3, PairView.tsx:29, and the scam-path/warning-copy finding). It also absorbs the "Pair again on unauthenticated pc_revoked" sub-point only as a related prompt to fix. Raised to high: unauthorized Control from a screenshot, against an audience that streams.
- **F5** merges: "One attacker can crash the relay… OOM in about 9 sockets", "Relay container mem_limit 128m…", and "Any client can OOM-kill the shared relay…".
- **F7** merges the two snapshot-over-256-KiB findings (mod.rs:735 and :736). The unchecked preview JPEG size is covered under F28.
- **F8** merges the three same-origin findings (storage.ts:47 ×2 and storage.ts:46 with the WEB_BASE_URL freeze).
- **F9** merges: "Snapshot sends vault-decrypted private-server link/access codes…" (medium), "Snapshot overshares to every paired device…" (low), and "View-only devices get free-text account notes…" (low).
- **F10** merges: "Held keys, right mouse button and touch contacts stay pressed…" (remote_input.rs:115) and "No viewer liveness on the PC…" (mod.rs:964).
- **F11** merges the three "Disconnect all" findings (medium plus two low).
- **F15** merges the two rate-limit and IPv6 brute-force findings.
- **F16** merges the pairing-blob reflection finding and the unvalidated-ack finding.
- **F17** merges the PC-side handshake replay finding (low) and the browser-side one (info).
- **F18** merges the two revoked-device online-oracle findings.
- **F24** merges the two reconnect-jitter findings (mod.rs:570 and :502).
- **F36** merges "Docker hardening gaps" and "alpine:3.21 is past end of support".
- **F37** merges "Custom relay URL feature can't work…" (low) and "Site-wide CSP blocks ?relay=…" (info).
- **F42** merges the two vitest advisory findings.

## 4. Residual risks to document, not fix

- **Whoever controls the VPS can serve malicious dashboard code.** The relay and the dashboard JS are on the same Hostinger VPS behind the same Caddy, so the relay is "blind" only against a passive operator. A root compromise of the VPS can serve modified dashboard JS that uses every returning user's stored key and reads the QR `#pair=` secret (web/remote/src/App.tsx:12-18). SRI cannot help, because the HTML that would carry the hashes comes from the same server.
  - Document it in the README, the spec and the in-app copy.
  - Fix the false claim at web/remote/src/components/PairView.tsx:184 ("RoALT's relay sees only encrypted bytes"), and qualify it until F1/F2 land.
  - Reduce the risk by hosting the static dashboard on separate CI-built infrastructure and on its own origin (F8). The PC-side approval stays the trust anchor.
- **Control means account-level power even though cookies never leave the PC.** Launching into any place or private server, and touch input into purchase or trade UIs, can move Robux and items. The pairing copy should say this honestly ("Control can launch your accounts and tap inside games") instead of only "Cookies and vault passwords never leave this PC".
- **The relay sees metadata.** It sees the PC's IP, a stable pcId, online times, device counts and which PCs each device subscribes to, plus traffic volume. F38 and F22 reduce and disclose this but cannot remove it. Also state the Caddy and Docker log retention.
- **Version skew.** One deployed dashboard and relay must serve desktop builds that update only when users click Update. Even with F21, there will be a long tail of old builds. Document the support window.
- **Manual-code entropy.** The code space is 31^8 ≈ 2^39.6 and the code is valid for 120 s. Even after F15, a distributed guesser can in theory hit some user's open room. Once F14 (approve must match the SAS on the phone) and F1/F2 are in, a hit is harmless, so treat this as accepted risk.
- **Windows 10 yellow capture border.** It cannot be removed on Windows 10 and is the only OS-level cue there. Document that previews show it, and that Windows 11 shows nothing (see F30).

## Appendix: all verified findings (before merging)

Each was read against the code by its auditor and then upheld by adversarial verifiers (3 independent verifiers for critical/high).

### A1. Manual-code pairing never authenticates the browser's identity key or deviceId: a relay can swap them and pair its own key
*critical* (claimed critical) · `src-tauri/src/remote/mod.rs:1100` · area: crypto

**What:** In manual pairing the browser sends `{mode:"manual", ephemPubB64, deviceId, devicePubB64, label}` as plain base64 JSON inside `pair_join` (web/remote/src/transport.ts:533-540). The PC copies `env.device_id` and `dev_pub_vec` straight into `PendingManualPair` (mod.rs:1111-1130). The SAS and pairKey come only from ECDH over the ephemeral keys, with salt = pairId (crypto.rs:311-326, crypto.ts:201-214). Nothing ties `devicePubB64` or `deviceId` to the SAS or to the encrypted channel. On approval, `complete_pairing_and_ack` stores whatever key arrived (mod.rs:361, 447-455). The UI default role is Control (src/features/remote/PairDialog.tsx:39). The text at web/remote/src/components/PairView.tsx:184 claims the relay sees only encrypted bytes, which is false for this message.

**Attack:** Anyone running or compromising the relay (one Docker container or Caddy on one VPS serves every user) rewrites `devicePubB64` in the `pair_join` to an attacker key and leaves `ephemPubB64` unchanged. Both screens show the same SAS, so the user compares the digits and approves. The PC now trusts the attacker key under that deviceId with the chosen role. The relay is also the roomToken gatekeeper, so it can send `viewer_joined{deviceId}` to the PC whenever it likes. It then passes `verify_handshake` (mod.rs:721) with the attacker key and gets a full Control RPC session: launching and closing clients, input injection, previews. The real browser receives a valid ack but every handshake it sends afterwards fails, so the user only sees the PC as 'offline'. Setting `deviceId` to an already-paired Control device's id makes `remote_device_upsert` overwrite that device instead. Every PC that pairs by manual code while the relay is compromised is taken over, which breaks the 'relay is blind' promise.

**Fix:** Never let device identity travel outside the authenticated channel. (a) After the SAS exchange, the browser sends `{deviceId, devicePubB64, label}` sealed under pairKey, plus an ECDSA signature by the device identity key over `"roalt-remote-v1-pair-bind" || pairId || browserEphemPub || pcEphemPub || deviceId`. The PC verifies the signature before it shows the approval card. (b) Feed both ephemerals and H(deviceId||devicePub) into the HKDF salt or info that produces the SAS and pairKey. (c) `pair_approve` should take the SAS the user saw, or a pending nonce, and reject it if `pending` changed. Today a re-join from the same viewer conn silently replaces `pending` (mod.rs:1123) while the card is on screen.

### A2. Manual-code SAS has no commitment: whoever sits on the pairing path can grind a matching 6-digit SAS and MITM the pairing, getting a persistent Control device
*critical* (claimed critical) · `src-tauri/src/remote/crypto.rs:311` · area: abuse

**What:** derive_manual_pair_keys (crypto.rs:311-326) builds the SAS as HKDF(ECDH(pc_ephem, browser_ephem)) mod 10^6. Neither side commits to its ephemeral key first. The browser sends its ephemeral key, device public key, deviceId and label in plaintext (transport.ts:532-540). The PC answers with its ephemeral key in plaintext (mod.rs:1116-1144). The relay also gets the manual code in plaintext in pair_open (mod.rs:849-851), so it can join any room by itself. Attack: the man in the middle joins the PC's room with its own key A1. The PC replies with P and fixes SAS_pc = f(A1,P), which the MITM can compute. The MITM then answers the real phone with A2, chosen so that f(B,A2) == SAS_pc. That takes about 1e6 tries on average. My measurement was 0.23 ms per try through Node WebCrypto, so about 4 minutes on one thread, a few seconds on 16 cores, and far less in native code with incremental point addition (A2+G gives a new key for one point addition). Both screens then show the same SAS. The user approves, and the PC stores the attacker's device_pubkey (inside the substituted plaintext envelope) with the chosen role, default Control. Meanwhile the attacker sends the phone an ack sealed with its own pairKey, holding an attacker pcPubB64, so the phone pairs with a fake PC. SAS comparison is the only thing that protects manual pairing, and here it gives no protection.

**Attack:** (d) A compromised relay or the relay operator does this for every manual pairing on wss://roalt.dev, live, with no trace. (b)/(a) Without any relay compromise, a stranger who sees the 8-char code (stream, screen share, a pasted code) does the same thing using the pair-room overwrite bug below. Result: a permanently paired Control device on the victim PC, while the victim believes they paired their own phone. It can launch all accounts into any place, inject taps into the clients (purchase prompts, trades), and kill clients or shut the PC down.

**Fix:** Add a commitment round. The browser first sends H(browser_ephem || nonce), the PC then sends its ephemeral key, and only then does the browser reveal browser_ephem and nonce. The PC checks the hash before it derives or shows the SAS. Equivalently, bind the SAS to a transcript the MITM cannot adapt, as in ZRTP/Bluetooth numeric comparison. Also mix both device static public keys and the pcPub into the SAS HKDF info, and send the relay only H(manual_code) instead of the code itself. Until this ships, document that manual pairing is not safe against an active relay and point users to QR.

### A3. 6-digit SAS has no commitment, so a MITM can grind a matching SAS in seconds and own both sides of manual pairing
*high* (claimed high) · `src-tauri/src/remote/crypto.rs:311` · area: crypto

**What:** The browser reveals its ephemeral public key first (transport.ts:533-540). The PC then answers with its ephemeral key (mod.rs:1116-1144). Each side computes SAS = HKDF(ECDH, salt=pairId, info="roalt-remote-v1-sas") mod 1e6, about 20 bits (crypto.rs:321-325, crypto.ts:210-212). Nothing commits the first sender before the second one picks its key. A man in the middle runs two separate ECDH exchanges. Once it knows the PC-side SAS, it can search freely for a browser-facing ephemeral that produces the same 6 digits. I measured a naive Node loop at 124 µs per try (keygen + ECDH + HKDF), so the expected 1e6 tries take about 8 s on 16 cores. Using m+i incremental point addition is much faster. Either way it fits easily inside the 120 s pairing window.

**Attack:** Compromised relay: it forwards E_M1 to the PC, receives E_PC and computes SAS_PC. It then grinds m2 until SAS(m2·E_B, pairId) == SAS_PC and sends E_M2 to the browser as `pcEphemPubB64`. Both screens show the same digits and the user approves. The relay now holds both pairKeys. It decrypts the real ack (roomToken, pcPubB64), re-seals an ack for the browser that carries its own `pcPubB64`, and registers its own device key on the PC. The result is Control of the PC plus a permanent, invisible MITM of every later session from that browser: snapshots, previews and commands. Fixing the first finding (moving the device key into the sealed channel) does not stop this, because the sealed channel itself is MITM'd.

**Fix:** Use a commitment-based SAS (the ZRTP / Bluetooth numeric-comparison pattern). The browser first sends only commit = SHA-256("roalt-commit" || E_B || pairId). The PC replies with E_PC. Only then does the browser reveal E_B, and the PC checks it against the commitment. Derive the SAS from HKDF over (E_B || E_PC || pairId || deviceId || devicePub). A MITM then gets one 1e-6 guess per pairing attempt. Mixing the manual code into a PAKE does not help against the relay, because the relay sees the code in `pair_open`.

### A4. One attacker can crash the relay for every user by pairing with themselves and leaving a viewer that never reads (OOM in about 9 sockets)
*high* (claimed high) · `server/remote-relay/src/lib.rs:383` · area: relay

**What:** The outbound queue is bounded by message count, not bytes. OUTBOUND_CAP = 64 and CONTROL_RESERVE = 8 (lib.rs:36-38, channel created at :330), so a target socket can hold 56 routed binary frames of up to MAX_BINARY_FRAME (256 KiB) each. That is about 14 MiB per viewer, plus the tungstenite write buffer and the frame in flight. Routing only checks `viewer.tx.capacity() > CONTROL_RESERVE` (:383-385 for PC->viewer, :402-404 for viewer->PC). It never checks queued bytes, never checks how long the queue has been full, and never closes a sink that stays stuck. Nothing limits how many viewers one PC can have attached: `attached_viewers` is unbounded and many sockets can subscribe with the same token. A 'PC' costs nothing to create, since any random 64-hex pcSecretHash gives a valid pcId (:442-449). Its allowedTokens can be any value the attacker picks (:451-456), so the attacker can make a valid PC/viewer pair with no SAS and no desktop app. docker-compose.yml sets mem_limit: 128m. I confirmed this with a throwaway test on a copy of the crate (scratchpad relaycopy, test self_paired_slow_viewer_buffers_megabytes): one self-paired PC sending 256 KiB frames to one viewer that does not read left 56 frames (about 14 MiB) queued in the relay. Bad luck at scale has the same effect: a phone that stops reading while subscribed to 4 FPS previews keeps 56 JPEGs (several MB) queued. About 30 stuck phones at once reach the same OOM.

**Attack:** The attacker opens 1 WS as a PC (pc_hello with sha256 of any string, allowedTokens=[sha256('t')]). They open 10 WS as viewers, each subscribing to that pcId with roomToken 't', and those viewers never read their sockets. The PC socket then pushes 256 KiB binary frames addressed to each viewerId. After about 130 MB of upload the container passes 128 MiB and the kernel OOM-kills it. Every RoALT PC and phone worldwide disconnects. restart: unless-stopped brings it back, and the attacker repeats this every few seconds.

**Fix:** Make backpressure work on bytes. Keep a per-socket AtomicUsize of queued bytes, increased on enqueue and decreased in the writer. Drop binary frames once a viewer has more than about 1 MiB queued (a PC about 2 MiB). If a socket stays over its budget for more than about 10 s, close it. Add a global queued-bytes budget, for example 64 MiB, above which all binary frames are dropped. Cap attached viewers per PC (for example 16) and per token (for example 2). Set axum `write_buffer_size`/`max_write_buffer_size` explicitly. Lower MAX_BINARY_FRAME for viewer->PC traffic, which is only commands and handshakes, to something like 16 KiB.

### A5. No idle timeout, liveness check or connection cap: idle sockets exhaust the relay, and dead phones keep PCs streaming previews
*high* (claimed high) · `server/remote-relay/src/lib.rs:343` · area: relay

**What:** `while let Some(Ok(msg)) = ws_rx.next().await` has no timeout. A socket that never sends pc_hello, subscribe or pair_join (Role::Unregistered) lives forever, and so does a half-open TCP peer. The relay never sends WS pings and never closes a peer that stops sending. Both real clients already send {"t":"ping"} every 25 s (web transport.ts:168-170, desktop mod.rs:643/936), but the relay only answers them (:433-435) and does not use them to detect liveness. Nothing caps connections per IP or globally (ws_handler :144-162 always upgrades). Each socket costs two tokio tasks, a 64-slot channel and a tungstenite codec that pre-reserves a 128 KiB read buffer (tungstenite 0.29 default, BytesMut::with_capacity). The relay reads each message in full, up to 257 KiB including text (:159-160), before applying the 32 KiB text check at :346. Some per-connection work also scales with total relay size: every disconnect write-locks and scans all pair rooms (:785-789), and every pc_hello scans all viewers (:471-480). An attacker with many cheap rooms or sockets makes every connect and disconnect slower for everyone. Bad luck at scale: when a phone sleeps or loses mobile coverage mid-stream, the relay keeps the viewer attached until Caddy's TCP gives up, which can be around 15 min with unacked data. The PC never gets viewer_left, so it keeps encoding and uploading previews at up to 4 FPS into a queue that drops them, and the desktop 'Remote · N' badge shows ghost viewers.

**Attack:** One host, or one IPv6 /64 (roalt.dev has AAAA 2a02:4780:c:5355::1), opens tens of thousands of WSS connections and sends nothing, or trickles a 250 KiB frame into each one. The relay runs out of file descriptors (accept errors stall all new connections) or memory, and every user's PC goes offline. No pairing or knowledge of any user is needed.

**Fix:** Wrap each read in tokio::time::timeout. Close the socket if it does not register (pc_hello, subscribe or pair_join) within about 10 s, and close it if it receives nothing, including the existing 25 s app pings, for about 70 s. Also send WS pings from the relay. Add a global connection semaphore (for example 20k) and a concurrent-connection cap per source (IPv4 /32, IPv6 /64), returning 429 before the upgrade. Track the one pair room a viewer joined in its connection state instead of scanning all rooms in cleanup, and index waiting viewers by pc_id instead of iterating all viewers in pc_hello. Set `ulimits: nofile` in docker-compose (and check `cat /proc/1/limits` in the running container: recent Docker defaults to a 1024 soft limit, which would cap the relay at about 1000 users).

### A6. Manual-code pairing SAS does not bind the browser's identity key, and nothing is committed first, so the relay can enroll its own Control device while the SAS still matches
*high* (claimed high) · `web/remote/src/transport.ts:532` · area: web

**What:** The manual pair_join envelope (transport.ts:532-541) is sent as plain base64 JSON. It carries ephemPubB64, deviceId, devicePubB64 and label. The SAS and pairKey come only from ECDH(browser_ephem, pc_ephem) with salt=pairId (crypto.ts:201-214, mirrored in src-tauri/src/remote/crypto.rs:311-325). deviceId, devicePubB64, label and the PC's static key are not part of the SAS input. The PC stores whatever devicePubB64/deviceId came in the plaintext envelope (src-tauri/src/remote/mod.rs:1119, 1135) and enrolls it when the user clicks Approve (pair_approve, then complete_pairing_and_ack, which calls remote_device_upsert). The browser never sends anything back under pairKey that would confirm its identity key. There is also no commit-then-reveal step: the browser shows its ephemeral key first and the PC replies with its own (mod.rs:1126-1150).

**Attack:** Precondition: the relay is malicious, i.e. the relay process or container is compromised, or someone runs a custom relay. This is the adversary that E2EE and the SAS are meant to stop. (1) Easy variant: the relay passes the browser's ephemPubB64 through unchanged but swaps in its own devicePubB64 and deviceId. Both sides derive the same ECDH secret, so the SAS on the phone and on the PC match exactly. The user approves, and the PC enrolls the attacker's key with the chosen role, by default Control. The relay can then send ViewerJoined{device_id: attacker} itself, sign the session handshake with its own key, and get a full Control session. That allows launching and closing clients, inputKey/inputTouch SendInput, live previews, shutdown and panicKill, with no roomToken needed, because it is the relay. (2) Seamless variant: the relay sends its own ephemeral M1 to the PC, learns the PC's SAS, then tries about 10^6 candidate keys M2 until the browser's SAS matches. That takes seconds on a few cores, well inside the 120 s browser timeout. It then also holds the pairKey, decrypts the ack (roomToken, pcPub) and can re-seal a forged ack, giving a persistent MITM on both sides. The relay can also start a re-pair on demand: it sends the unauthenticated plaintext {t:'pc_revoked'} (transport.ts:242-247), and App.tsx:202-224 then shows 'Access revoked' with a 'Pair again' button. Impact: the 'relay is blind' and SAS promises fail for every manual-code pairing on the single shared relay, which can lead to takeover of every PC paired this way.

**Fix:** (a) Stop trusting identity fields that arrive in plaintext. After manual_challenge, the browser sends {deviceId, devicePubB64, label} as a blob sealed under pairKey (sealPairBlob). The PC enrolls only the values it decrypts from that blob, and ignores the plaintext envelope apart from ephemPub. (b) Bind the whole transcript into the SAS: info = 'roalt-remote-v1-sas' || browser_ephem || pc_ephem || devicePub || deviceId || pcStaticPub. (c) Add a hash commitment so the relay cannot try keys until the SAS matches. The browser first sends C = SHA256(browser_ephem || devicePub || nonce), the PC replies with pc_ephem, then the browser reveals the values and the PC checks C before showing the SAS (as in Bluetooth LE numeric comparison or ZRTP). (d) Show the 'Access revoked' screen only after an authenticated signal, or at least do not prompt 'Pair again' when only the relay says so.

### A7. Relay container mem_limit 128m with no per-connection byte, connection or write-timeout caps: one script (or slow phones at scale) OOM-kills the shared relay
*high* (claimed high) · `server/remote-relay/docker-compose.yml:11` · area: release

**What:** The compose file sets `mem_limit: 128m` (docker-compose.yml:11) on the one relay that serves every user. The relay's outbound queue holds up to OUTBOUND_CAP = 64 messages per connection (lib.rs:36, lib.rs:330). Routed binary frames can be up to 256 KiB each (MAX_BINARY_FRAME, lib.rs:26), and the only check before queueing is `capacity() > CONTROL_RESERVE` (lib.rs:383). So a viewer that stops reading holds about 56 x 256 KiB, roughly 14 MiB, in relay memory, plus axum/kernel buffers. There is no per-IP connection limit (rate limiting only covers pair_join), no limit on viewers per PC, and no write timeout that drops a stalled writer. Anyone can be both ends: send pc_hello with a self-made secret and allowedTokens, then open viewers that subscribe with that token. Nothing ties this to a real RoALT install.

**Attack:** An attacker opens 1 PC connection and about 10 viewer sockets to it, and none of the viewers ever read. The PC pushes 256 KiB binary frames. Within seconds the container goes past 128 MiB, the kernel OOM-kills it, Docker restarts it (`restart: unless-stopped`), every user's PC and phone is dropped, and the in-memory pairing rooms are lost. Looping this keeps Remote down for everyone at little cost. No attacker is needed at scale either: phones on slow cellular links watching 4 FPS 640px previews each queue a few MiB, so a few dozen of them at once reach the same OOM.

**Fix:** Budget queued bytes per connection, not message count (e.g. 2 MiB). Drop preview frames above that budget, and close the socket if its writer has been stalled for more than about 10 s (wrap ws_tx.send in tokio::time::timeout). Cap connections per IP (e.g. 32) and viewers per PC (e.g. 16). Size mem_limit from those caps times the expected number of users (e.g. 512m to 1g) and add pids_limit. Add a test that a non-reading viewer cannot grow relay RSS past its budget.

### A8. Relay lets any client that says it is a PC overwrite another PC's pairing room and manual-code mapping (verified with a PoC test)
*high* (claimed high) · `server/remote-relay/src/lib.rs:576` · area: abuse

**What:** In PairOpen, state.pair_rooms.insert(pair_id, room) and state.manual_codes.insert(code, pair_id) (lib.rs:576-579) run with no check that the pair_id or code is free or belongs to this PC. Anyone can register as a PC for free (random secret, then pc_hello), then send pair_open with a victim's manualCode or pairId. The relay then sends later pair_join attempts for that code to the attacker. I confirmed this with a throwaway integration test against a copy of the relay (scratchpad relaypoc/tests/poc.rs). After a stranger PC re-opened 'ABCD-2345', the victim phone's join reached the attacker PC. Overwriting the pairId of a QR room worked the same way. The PC's pending slot is also first-come: the relay marks the room 'busy' for everyone else (lib.rs:667-671), and the real phone then sees the generic 'Invalid pairing code' message (transport.ts:548-553) while the desktop shows the attacker's request.

**Attack:** A streamer or a scam victim on a 'support' screen share shows the manual code. The attacker joins the real room with that code, so the desktop now shows a pending request labelled 'iPhone · Safari'. The attacker then re-opens the same code from a fake PC, so the victim's phone lands in the attacker's room. Together with the SAS grind above, both screens show matching digits. The victim approves, and the attacker gets Control of the PC. A plain stranger can do this with no relay compromise. Without the grind it still lets anyone hijack a pairing or squat a code (DoS).

**Fix:** In PairOpen, reject (pair_err 'busy') if the pair_id or manual code is already registered by a different pc_id or conn. Use DashMap entry().or_insert semantics so the check and insert are one step. Map the manual code to the room only through a per-PC record. Add a test that a second PC cannot claim an existing code or pairId.

### A9. Any client can OOM-kill the shared relay (128 MB container) with its own PC plus a few viewers that never read
*high* (claimed high) · `server/remote-relay/src/lib.rs:383` · area: abuse

**What:** Each socket has a 64-message outbound queue (OUTBOUND_CAP, lib.rs:36), and routed binary frames may fill 56 of those slots (capacity() > CONTROL_RESERVE, lib.rs:383-385 and 402-404). Each frame can be up to MAX_BINARY_FRAME = 256 KiB, so one stalled receiver pins about 14 MiB. Becoming a PC needs no permission (random secret, then pc_hello), the PC sets its own allowedTokens, and many viewer sockets can use the same token. The container has mem_limit 128m (docker-compose.yml:11). There is also no per-IP connection cap, no auth timeout for Unregistered sockets, and PcHello scans every viewer on every PC registration (lib.rs:471-480).

**Attack:** One machine registers a fake PC, attaches about 10 viewer sockets with its own token, stops reading on them, and has the fake PC push 256 KiB binary frames. About 140 MB is queued, Docker OOM-kills the relay, and every RoALT user worldwide is disconnected. The attacker repeats this after each restart.

**Fix:** Budget per connection in bytes, not messages (for example 1-2 MiB queued, then drop frames or close the slow consumer). Cap viewers per PC and sockets per IP (relay plus Caddy), close sockets that do not register within about 10 s, and set a global memory budget below the container limit. Index viewers by subscribed pc_id so PcHello does not scan all viewers.

### A10. QR pairing grants Control immediately, with no desktop confirmation
*medium* (claimed medium) · `src-tauri/src/remote/mod.rs:1092` · area: crypto

**What:** Any `pair_join` whose blob decrypts under the QR key is completed on the spot with `RemoteRole::Control` (mod.rs:1086-1096 → complete_pairing_and_ack). The user never approves it or picks a role. The only possession factor is the QR on screen, valid for 120 s, and the first joiner wins the room (relay busy check).

**Attack:** A Roblox streamer, a Discord screen-share, an OBS capture of the full desktop, or a screenshot posted in a support channel shows the QR (or its URL) during the 120 s window. A viewer scans it and silently gets a Control device: input injection into game windows, launching and closing clients, live previews. The owner's own phone gets 'busy'. The desktop only adds a row to the device list.

**Fix:** Send QR joins through the same pending → Approve step as manual codes. Show the label and default to View role. The QR secret already authenticates the channel, so no SAS is needed. As a minimum, default QR pairings to View and raise a prominent desktop notification.

### A11. Any PC can take over another PC's manual code or pairId, sending the victim's phone into the attacker's pairing room
*medium* (claimed medium) · `server/remote-relay/src/lib.rs:576` · area: relay

**What:** PairOpen inserts into `pair_rooms` and `manual_codes` without checking whether the pairId or manual code is already in use by a different PC (:569-579). DashMap::insert overwrites the existing entry. remove_pair_room (:763-767) also deletes `manual_codes[code]` without checking that the code still points at that room. Manual codes are client-chosen, and a free attacker 'PC' can register any code. I confirmed both with throwaway tests: manual_code_can_be_rebound_by_another_pc (a phone typing the victim's code was delivered to the attacker PC's room) and pair_id_of_another_pc_can_be_overwritten.

**Attack:** A victim shows their 'ABCD-2345' code on a stream, a screen share or a help screenshot. The attacker first pair_joins with that code, which takes the victim's real room and makes the victim's desktop show an Approve prompt with SAS A. The attacker then pair_opens their own room with the same code. The victim's phone joins the attacker's room and goes through a normal-looking SAS flow, showing SAS B, instead of the 'busy' error that would hint at the problem. A victim who clicks Approve on the desktop without comparing the 6 digits gives the attacker's device a remote device entry on their PC with the role they pick, for example Control. Even without the race, rebinding a code silently breaks other users' pairing.

**Fix:** In PairOpen, reject with pair_err 'taken' if the pairId exists for a different pc_id, or if the manual code maps to a live room owned by another PC; the desktop then regenerates. Better, have the relay generate manual codes itself so they are unique. In remove_pair_room, remove the code mapping only if it still maps to this pairId (`manual_codes.remove_if(&code, |_, v| v == pair_id)`). On the desktop, consider requiring the user to type the phone's SAS instead of clicking Approve.

### A12. Relay is an unmetered bidirectional tunnel: no per-connection bandwidth or message-rate limits, and invalid frames do not close the socket
*medium* (claimed medium) · `server/remote-relay/src/lib.rs:361` · area: relay

**What:** The reader loop never waits on its target and has no token bucket. It accepts messages as fast as TCP delivers them, up to about 257 KiB each (:159-160), and forwards with try_send or drops them silently. Binary frames from Unregistered sockets (:407), to unattached targets (:375-377), with a bad version (:362-363), and any amount of control JSON (:345-359, including subscribe or ping spam that produces outbound replies) are all handled with `continue` and never close the socket. A self-paired PC/viewer pair (see the OOM finding) makes the relay a free data pipe. Bad luck at scale: legitimate previews are the main bandwidth cost and nothing caps them per PC or per viewer at the relay.

**Attack:** The attacker self-pairs and streams at line rate through roalt.dev, using up the VPS port bandwidth or Hostinger's monthly transfer quota, or uses it as a free proxy. Legitimate previews and commands for all users stall, and the provider may throttle or suspend the VPS.

**Fix:** Use a token bucket per connection on inbound bytes and messages (for example a PC at 2 MiB/s sustained with 8 MiB burst, a viewer at 64 KiB/s with 50 msgs/s, since viewers only send commands), closing the socket on sustained violation. Close sockets that send binary while Unregistered or to targets they are not attached to. Optionally add a per-PC egress cap that the previews feature is designed around.

### A13. Held keys, right mouse button and touch contacts stay pressed when the phone goes quiet; release events are refused while Windows is locked or after a downgrade to View
*medium* (claimed medium) · `src-tauri/src/instances/remote_input.rs:115` · area: dispatch

**What:** The input worker (remote_input.rs:115-133) only lets go of W/A/S/D/Space, the injected right mouse button (camera drag) and touch contacts in two cases: an explicit up/lift from the phone, or ReleaseAll. ReleaseAll is sent only on viewer_left, a decrypt error, revoke, DisconnectAll or the relay connection ending (mod.rs:561, 688-696, 755, 885, 908). There is no heartbeat or deadman timer, even though docs/REMOTE_DASHBOARD.md section 5.3 requires one ("automatic key-up release on >500 ms heartbeat drop"). Held touch contacts are re-injected every KEEP_ALIVE of 60 ms for as long as they are held (line 129). The relay has no read or idle timeout on viewer sockets (lib.rs:343, nothing near it), so a phone that drops off the network without closing the socket stays 'attached' until TCP gives up, which takes many minutes on Linux defaults. Two more paths leave input stuck. dispatch::input (dispatch.rs:242-247) rejects every input command, including down:false and what:"up", while session::locked() is true or the role is Readonly. When SyncTokens downgrades a device to View (mod.rs:894-897), its sessions keep their role update but nothing is released.

**Attack:** Plain bad luck at scale. A user holds the joystick and their phone loses signal, or goes into a pocket and the browser tab is suspended. On the PC, W and/or the right mouse button stay injected as down for minutes: the character keeps running in-game, and the physical user's own mouse acts as a right-drag in every app. The same happens if the PC auto-locks mid-hold (the phone's key-up gets a 'windows_locked' error) or if the desktop user switches that device to View mid-hold (the up gets a 'readonly' error).

**Fix:** Add a PC-side watchdog in the worker: record the last event time for each held key, finger and drag, and release anything not refreshed within about 750 ms (recv_timeout already ticks every 60 ms). Have the dashboard resend held keys and fingers every ~250 ms. In dispatch::input, always let release events through (InputKey down:false, Touch/Camera what:"up") whatever the lock state or role. Call remote_input::release_all() when the lock state changes (notify_lock_state_changed) and when SyncTokens changes any session's role.

### A14. A large snapshot exceeds the relay frame cap: the relay drops the PC's socket and the PC enters a reconnect loop, so Remote stops working for users with many accounts or clients
*medium* (claimed medium) · `src-tauri/src/remote/mod.rs:735` · area: dispatch

**What:** After every handshake the PC seals and sends the full snapshot as one frame (mod.rs:735-740; again on RequestSnapshot at 786-790 and on SyncTokens at 899-900). The snapshot has no size bound. All accounts go in, each a ~400-byte AccountSummary before notes (which can be up to 2000 chars) and FFlags; all instances go in at ~700 bytes each; crashes are capped at 40. The relay sets max_message_size(MAX_BINARY_FRAME + 1024) of 257 KiB (server/remote-relay/src/lib.rs:159-160). It silently drops binary frames over 256 KiB (lib.rs:362), and a frame over 257 KiB ends the read loop `while let Some(Ok(msg))` (lib.rs:343), which disconnects the PC. The PC then reconnects (mod.rs:559-578), the viewer re-joins, the same oversized snapshot is sent, and the cycle repeats. docs/REMOTE_DASHBOARD.md section 5.0 lists this as untested.

**Attack:** Bad luck at scale. A RoALT user with roughly 500 alt accounts, or a few hundred accounts plus dozens of running clients (typical for this app's farming audience), pairs a phone. The first snapshot is about 260 KB or more, and the relay closes the PC connection. Every viewer of that PC drops and the PC stays in an Error/reconnect cycle with up to 60 s backoff. Remote is unusable, and the only clue is a generic relay error.

**Fix:** Measure the encoded snapshot before sealing it. Shrink it with the redacted DTOs from the previous finding. If it is still over about 200 KiB, split it into chunks (for example Snapshot{part, total}, or AccountsUpdated pages of N accounts) and have the dashboard reassemble them. On the relay, answer an oversized frame with an error instead of dropping the whole PC connection.

### A15. QR pairing grants full Control with no confirmation on the desktop
*medium* (claimed medium) · `src-tauri/src/remote/mod.rs:1093` · area: dispatch

**What:** handle_pair_join's QR branch calls complete_pairing_and_ack(..., RemoteRole::Control) as soon as the blob decrypts with the QR secret (mod.rs:1086-1097). The desktop shows no approval step, no SAS and no role choice. Manual pairing, by contrast, waits for Approve plus a role (pair_approve, mod.rs:334-364). Anyone who gets the QR image or URL during its 120 s life becomes a permanent Control device: they can launch or move any account to any place, follow any user, close or kill all, schedule a shutdown, and inject touch and keys into clients. With touch input they can operate in-game trade and purchase UIs on every account.

**Attack:** Many RoALT users stream or screen-share (Twitch, Discord Go Live) while setting things up. A viewer scans the QR from the stream within 2 minutes and is silently paired as Control. The only trace is a new row in Settings → Remote and the title-bar pill when they connect. They then launch the user's accounts into a game of their choosing and use touch control to trade items away.

**Fix:** Route QR joins through the same pending → Approve flow as manual codes. Show the device label, keep the role picker, and default to View. Optionally skip the SAS for QR but still require the click. Mark the QR as sensitive in the UI (blur it until clicked, or hide it while a screen-capture or streaming app is detected).

### A16. Dashboard shares the roalt.dev origin with the marketing site and a proxied Umami tracker, so any same-origin script can use the stored keys and room tokens to control paired PCs
*medium* (claimed medium) · `web/remote/src/storage.ts:47` · area: web

**What:** The dashboard stores its identity CryptoKeyPair and the list of paired PCs (pcId, pcPubB64, roomToken, relayUrl) in IndexedDB 'roalt-remote-v1' on origin https://roalt.dev (storage.ts:4-5, 47-68). 'Non-extractable' only stops export. Any script running on the same origin can still open the DB and call crypto.subtle.sign with the stored key. The same origin also serves the main site (main.js, 404.js) and /u/script.js, an Umami tracker proxied by Caddy from a separate Next.js container. A GET of /u/script.js returned 'Via: 1.1 Caddy', Umami headers and a second CSP with 'unsafe-inline' 'unsafe-eval'. Every roalt.dev page except /remote/ loads that script. The site-wide CSP that /remote/ also receives allows connect-src wss://roalt.dev, https://discord.com, https://api.github.com and a workers.dev endpoint.

**Attack:** Any XSS on any roalt.dev page, or a compromised Umami container serving a modified /u/script.js, runs on the same origin for every visitor of the download site. For each visitor who has ever paired a phone or PC browser, it reads IndexedDB 'roalt-remote-v1', connects to wss://roalt.dev/ws/remote (allowed by CSP), sends subscribe with the stored roomTokens, and signs the viewer handshake with the stored identity key. It then holds a real E2EE Control session to every paired PC: live previews, launch and close, SendInput, shutdown. This is a takeover of many users at once that never touches the relay or the desktop app.

**Fix:** Serve the dashboard from its own origin (e.g. https://remote.roalt.dev) with no analytics and no other content. Give it its own strict CSP: default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; connect-src wss://roalt.dev; manifest-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'. Move existing users' pairings by re-pairing. Until then, at least stop loading /u/script.js anywhere on roalt.dev.

### A17. QR pairing completes on link open with no confirmation on either side, so anyone who sees the QR during its 120 s window gets Control
*medium* (claimed medium) · `web/remote/src/components/PairView.tsx:29` · area: web

**What:** PairView pairs automatically as soon as App.tsx:12-19 parses #pair=<id>.<secret> (PairView.tsx:29-47). The user is never asked to confirm, and no PC name is shown first. On the PC, the QR path calls complete_pairing_and_ack(..., RemoteRole::Control) right away, without the Approve step used for manual codes (src-tauri/src/remote/mod.rs:1100-1101). The QR stays on screen for PAIR_DURATION_MS = 120_000 (mod.rs:42). The other direction is also unconfirmed: any #pair link silently adds whatever PC sent it and selects it (App.tsx:55-62).

**Attack:** Many RoALT users stream or record their screens. A viewer pauses the stream, scans the QR or copies it from a screenshot, and opens it before the owner does. They instantly get a Control device on the streamer's PC, with no prompt anywhere. The owner only sees 'Pairing session not found'. In the other direction, an attacker sends a victim a roalt.dev/remote/#pair=... link for the attacker's own PC. The victim's dashboard silently pairs with it and selects it, which can be used to spoof a familiar PC name, and the attacker's PC learns the victim's long-lived deviceId and identity key.

**Fix:** On the PC, require the same Approve step for QR joins as for manual codes, showing the label from the sealed join, or enroll QR joins as View until approved. In the dashboard, show 'Pair with <pcName>?' using the PC name from the decrypted ack, and only call upsertPairedPc after the user confirms.

### A18. QR pairing grants persistent Control without any desktop approval
*medium* (claimed high) · `src-tauri/src/remote/mod.rs:1093` · area: desktop

**What:** handle_pair_join (mod.rs:1085-1098) finishes pairing as soon as any browser presents a blob sealed with the QR secret. It calls complete_pairing_and_ack with a hardcoded RemoteRole::Control. There is no pending state, no Approve click, and the role picker in PairDialog (PairDialog.tsx:98-106) only shows up in the manual-code branch. The QR is drawn in plain view on screen for 120 s (PairDialog.tsx:119-131), and nothing warns against showing it on stream. After a QR pairing, RemotePanel.tsx:50-55 closes the dialog silently, with no toast saying which device paired. RoALT's own model.rs notes that 'streamers show RoALT on screen'.

**Attack:** A user opens Pair device while streaming or screen-sharing on Discord. A viewer scans the QR off the stream (or a screenshot) before the user's own phone does. The attacker's browser is stored as a Control device and gets a room token and live E2EE sessions after that. From then on the attacker can see previews of every client, launch, close, move, rejoin and panic-kill clients, schedule a PC shutdown, and inject taps, keys and camera drags into the user's Roblox windows (trades, chat, purchases through the game UI). The dialog closes as if the pairing succeeded. The user's own scan then fails, they retry, and a stranger's device sits in the list unnoticed. The cookie promise still holds, but the View/Control promise does not.

**Fix:** Send QR joins through the same pending step as manual joins: store PendingManualPair with the QR pair_key and show the device label with Approve/Deny and the role picker. The QR secret already authenticates the channel, so no SAS is needed. Default the role to View. After any successful pairing, show a toast naming the device with an Undo/Revoke action. Blur the QR until hovered or clicked, with a short 'don't show this on stream' note.

### A19. Snapshot larger than 256 KiB kills the PC's relay connection, causing a reconnect loop for users with many accounts
*medium* (claimed medium) · `src-tauri/src/remote/mod.rs:736` · area: desktop

**What:** The full snapshot (every AccountSummary with notes up to 2000 chars, custom_flags, launch_args and profile, plus groups, favourites, instances and 40 crashes; dispatch.rs:21-57) is sealed and sent as one binary frame with no size check (mod.rs:734-741, 786-789, 899-900). AccountsUpdated sends all accounts the same way (mod.rs:915-921). The relay sets max_message_size and max_frame_size to 256 KiB + 1 KiB (relay lib.rs:159-160). A larger frame is a tungstenite capacity error, so `while let Some(Ok(msg))` ends and the whole PC socket is torn down, not just the frame. The doc lists 'snapshot size with many accounts' as untested.

**Attack:** No attacker needed. A multi-account user with a few hundred accounts, or about 100 accounts with long notes, pairs a phone. Each viewer handshake sends a too-large snapshot, the relay drops the PC, the PC reconnects, the phone re-attaches, the snapshot is sent again, and the cycle repeats with backoff up to 60 s. Remote never works for that user, every other viewer of that PC is cut off too, and the relay absorbs the churn. The same happens on any account change (BroadcastAccounts).

**Fix:** Before sending any frame, check that the sealed length plus 17 header bytes fits under 256 KiB. Split accounts into paged events (AccountsPage {offset, total, items}). Leave out or truncate fields the dashboard doesn't need (notes, launch_args, custom_flags, profile), or compress. If a frame would still be too big, send an explicit error event instead of the frame.

### A20. No viewer liveness on the PC: a vanished phone keeps capture (yellow border on Win10), JPEG encoding and held keys/RMB running for minutes
*medium* (claimed medium) · `src-tauri/src/remote/mod.rs:964` · area: desktop

**What:** The PC stops previews and releases input only on viewer_left, kick or decrypt failure (mod.rs:688-696, 750-759). The viewer's pings go to the relay only (transport.ts:167-170), not end-to-end to the PC. The relay has no read or idle timeout (lib.rs:343, plain `while let Some(Ok(msg))`), so a phone that drops off the network without a FIN is detected only when TCP retransmissions give up, about 15 minutes with Linux defaults behind Caddy. Meanwhile preview_tick keeps requesting captures of up to 16 windows at up to 4 fps and encoding JPEGs for that dead viewer (mod.rs:964-973, 1015-1073). The remote_input worker has no dead-man release for held keys or the right-button camera drag (remote_input.rs:110-130 only re-sends touch contacts), although the doc promises 'automatic key-up release on >500 ms heartbeat drop'. A role downgrade to View (mod.rs:894-897) and a Windows lock do not call release_all either.

**Attack:** Bad luck at scale: a user drives the joystick from their phone (W held) or drags the camera (RMB held) and walks into a dead zone, or the battery dies. On the PC, W and Space stay pressed and the character walks off or keeps jumping. The right mouse button stays logically down, which upsets the person physically at the PC. On Windows 10 the yellow capture border stays around their games, and CPU/GPU keep capturing and encoding for a viewer who is gone. The title pill keeps saying someone is watching.

**Fix:** Add an E2EE heartbeat from viewer to PC every 5 s. Drop a session's previews and input, and call release_all, after about 15 s with no inbound frame from that viewer. In the remote-input worker, auto-release held keys and a drag when no refresh has come within about 1 s, with the dashboard re-sending held state while a key is down. Also call release_all when a device's role changes and on WTS_SESSION_LOCK.

### A21. Snapshot sends vault-decrypted private-server link/access codes, account notes and launch args to every paired device, View-only included
*medium* (claimed medium) · `src-tauri/src/remote/dispatch.rs:25` · area: release

**What:** build_snapshot is documented as 'Never reads or exposes cookies or vault secrets' (dispatch.rs:19-20). It puts the full `s.db.accounts()` (AccountSummary: notes, launchArgs, customFlags) and `commands::favourites_list(...)` into RemoteSnapshot (dispatch.rs:25-32, 42-49). Favourites are stored vault-encrypted (commands.rs:3238-3239), and favourites_list (commands.rs:3231) decrypts them into LaunchTarget, which carries `privateLink.linkCode` and `privateAccess.accessCode` (src/bindings/LaunchTarget.ts). mod.rs:918 also pushes accounts in AccountsUpdated. Role does not filter any of this, so a device paired as View gets the same data. Elsewhere the product treats link codes and launch arguments as secrets: PRODUCT.md says link codes never reach logs or persisted UI state, README.md:443 says launch arguments are never written to logs, and README.md:497 says private server codes are never put in share links. Free-text notes are where users often keep passwords or recovery hints.

**Attack:** A user pairs a friend's or shared phone as View-only to let them watch the clients. That device now holds every private server link code (anyone with it can join or buy into those servers), every account's notes, and its launch arguments, readable in devtools. The same data goes to any script on the dashboard origin (see the same-origin finding).

**Fix:** Build a remote-specific DTO instead of reusing UI types. Send favourites as {id, name, kind, placeId, accountIds} without linkCode or accessCode; the PC resolves the target by id on LaunchFavourite. Drop notes, launchArgs and customFlags from the remote AccountSummary, or send them only to Control with an explicit opt-in. Add a test that serializes RemoteSnapshot and asserts that no linkCode, accessCode, notes or launchArgs keys appear.

### A22. QR pairing grants full Control instantly with no desktop confirmation, and the pairing UI never warns that the QR/code hands over control (scam path)
*medium* (claimed high) · `src-tauri/src/remote/mod.rs:1093` · area: abuse

**What:** handle_pair_join completes any join that decrypts with the QR secret right away, with RemoteRole::Control and no user step (mod.rs:1086-1097). RemotePanel then quietly closes the dialog once pairing clears (RemotePanel.tsx:50-55), with no toast showing which device was added. A screenshot or photo of the QR, or a frame of a screen share or stream, is enough for 120 s. For manual pairing the default role is Control (PairDialog.tsx:39), the dialog offers a Copy button for the code (PairDialog.tsx:64-72,126), and the approve screen shows the attacker-chosen label in the SAS line ('Check that this code matches the one shown on {name}', en.json:1892). sanitize_label strips only Cc controls, so bidi and zero-width characters get through (mod.rs:419-427). The only security copy at the decision point reassures the user instead of warning them: 'Cookies and vault passwords never leave this PC, even with Control.' (en.json:1894, PairDialog.tsx:108). Nothing says 'anyone with this code or QR can control your Roblox clients; RoALT will never ask for it'. Control is in practice account-level power: Launch/BulkLaunch with FollowUser/Place/PrivateServer can send every alt into an attacker's game (dispatch.rs:109-122), and InputTouch taps anywhere in the client window (remote_input.rs:142, 176-185), which covers Robux purchase prompts and in-game trade accept buttons. The cookie never leaves the PC, but the Robux and items it protects can.

**Attack:** "Send me a pic of your RoALT Remote QR for free Robux", or a Discord 'support' screen share, or a streamer opening Pair device on stream. The scammer scans it and gets a silent Control pairing. They bulk-launch all alts into their own place, tap Buy on developer-product prompts or accept trades through the live preview, so the Robux go to the scammer's game, and then schedule a PC shutdown to cover their tracks.

**Fix:** Require a desktop confirmation for QR joins too: show the incoming device label and approve/deny, as manual does (a 1-tap approve keeps QR fast). Default new devices to View only, and put live input behind a separate, explicit 'Allow touch control' permission. Add prominent copy in the pair dialog: 'Only pair your own devices. Anyone who sees this QR or code can control your Roblox clients. RoALT staff will never ask for it.' Remove the Copy button or warn when it is used. Replace the 'secretsStay' reassurance with an honest line ('Control can launch your accounts and tap inside games'). Toast and log every new pairing. Strip Unicode Cf/bidi characters from labels.

### A23. 'Disconnect all' (title-bar pill) is not durable: a kicked device re-attaches immediately or on the next PC reconnect
*medium* (claimed medium) · `src-tauri/src/remote/mod.rs:905` · area: abuse

**What:** DisconnectAll only sends kick_viewer (mod.rs:905-914). The relay's KickViewer removes the viewer from attached_viewers but keeps its subscription and the token stays allowed (lib.rs:616-629). Any later subscribe re-attaches it (lib.rs:713-724), and the next pc_hello re-attaches every subscribed viewer whose token is still valid (lib.rs:471-475). Yet the pill reads 'Connected: {names}. Click to disconnect all.' (en.json:1904, RemoteBadge.tsx:16-31), which makes it look like the emergency stop.

**Attack:** (c) A user notices an unknown 'Remote · 1' and clicks the pill. A malicious device sends subscribe again within milliseconds and goes on watching or controlling. Even a passive one comes back the next time the PC's connection to the relay cycles.

**Fix:** Make Disconnect all a real stop: turn Remote off, or pause it and refuse viewer_joined for N minutes, and offer 'Revoke all devices' next to it. Change the tooltip to say what it does. On the relay, drop the kicked viewer's subscription so it is not re-attached automatically.

### A24. Dashboard keys and room tokens share the roalt.dev origin with the marketing site and the Umami analytics proxy
*medium* (claimed medium) · `web/remote/src/storage.ts:47` · area: abuse

**What:** The non-extractable P-256 identity and all room tokens live in IndexedDB 'roalt-remote-v1' (storage.ts:4, 47-68) on the https://roalt.dev origin. Path scoping (/remote/) does not isolate IndexedDB. The same origin serves the marketing site (main.js) and /u/script.js, which is proxied to a separate Umami app (live response carries Umami's own CSP with 'unsafe-eval' 'unsafe-inline'). Non-extractable keys can't be exported, but any same-origin script can still use them: open wss://roalt.dev/ws/remote (allowed by connect-src), sign the handshake and drive every PC the browser has paired. The site-wide CSP also allows connect-src https://discord.com and a workers.dev endpoint, which can be used to exfiltrate tokens or snapshot data.

**Attack:** An XSS on any roalt.dev page, a compromised main.js, or a compromised Umami container serving a modified script.js runs on the roalt.dev home page. Paired users visit the home page in the same browser, and the script silently opens Control sessions to their PCs.

**Fix:** Serve the dashboard from its own origin (for example remote.roalt.dev) that hosts nothing else and has a dedicated strict CSP (connect-src only the relay, no third-party hosts, require-trusted-types-for 'script'). Migrate pairings by re-pairing.

### A25. Relay and dashboard JS are served from the same VPS, so the relay is 'blind' only against a passive operator
*medium* (claimed medium) · `web/remote/src/App.tsx:12` · area: abuse

**What:** The dashboard's code (index.html with Cache-Control: no-cache, unpinned hashed assets, no SRI) and the relay run on the same Hostinger VPS behind the same Caddy. A compromised VPS can serve modified JS. That JS reads the QR '#pair=' secret before it is scrubbed (App.tsx:12-18), uses the stored non-extractable keys for every returning user, and can rewrite pairing acks. SRI would not help, because the HTML that would carry the hashes comes from the same server. PairView tells users 'RoALT's relay sees only encrypted bytes' (PairView.tsx:184), and the spec describes the relay as blind (docs/REMOTE_DASHBOARD.md section 1). Neither holds against an active host.

**Attack:** (d) An attacker who gets root on the VPS ships one malicious bundle. Every user who opens roalt.dev/remote hands over Control of all their paired PCs. This is a mass takeover from a single compromise.

**Fix:** Treat this as residual risk and document it in the README, the spec and the in-app copy. Reduce it by hosting the static dashboard on different infrastructure than the relay (for example GitHub/Cloudflare Pages built by CI from tagged commits), so relay compromise is not code compromise, and use a separate origin. Publish the build hash and keep the PC-side SAS/approval as the trust anchor. Defence in depth: the desktop shows a toast for every new viewer session and every first input session.

### A26. A hostile paired PC can resolve or reject another PC's pending RPCs and pick the launch target used on that PC
*medium* (claimed high) · `web/remote/src/transport.ts:351` · area: gap:Web dashboard isolation between PCs: one paired PC acting against another PC in the same browser

**What:** Every PC's decrypted events go into handleEvent(rt, ev). The rpcResult case (transport.ts:351-358) looks up this.pendingRpcs.get(ev.reqId) and resolves or rejects that entry without checking p.pcId === rt.pc.pcId. PendingRpc stores pcId (line 56), but only rejectPending uses it. The map is keyed by a bare number (line 69), and reqIds come from one sequential counter shared by every PC (lines 70 and 436). A PC sees the reqId in each request it decrypts, so it can predict the next ones. For example, ClientsView's unmount cleanup sends subscribePreviews to the old PC (ClientsView.tsx:107), which tells PC B the exact counter value at the moment the user switches to PC A. The only RPC whose returned data the UI uses is targetParse. LaunchView fires it on every keystroke of the link box (LaunchView.tsx:57-58, onChange at 118) and stores res.parsed. The UI shows only `Ready to launch: {parsed.target.kind}` (LaunchView.tsx:132), never the placeId, jobId or link code. The Launch button then sends parsed.target to PC A in launch or bulkLaunch (LaunchView.tsx:70-76, 241-242). Rejections also put text chosen by B into the toast shown on A's page (LaunchView.tsx:80, ClientsView.tsx:137, MonitorView.tsx:23, ControlPad.tsx:36-43).

**Attack:** 1) The victim pairs a scammer's PC B in the same browser as their own PC A. 2) B runs a modified RoALT. It records the last reqId it received, then keeps sending small sealed rpcResult events for reqIds last+1 to last+200 over its valid E2EE session, e.g. {t:'rpcResult', reqId:n, ok:true, data:{parsed:{kind:'target', target:{kind:'privateLink', placeId:<scam place>, linkCode:<attacker's server>}}}}. 3) The user opens PC A, Launch tab, pastes a game link and taps 'Launch 12 accounts'. B's forged result arrives before A's real one, because A's must make the round trip through PC A, and the real one is then dropped since the pending entry is gone. 4) PC A launches all selected alts into B's game, server or followUser target. B links all of the victim's alt usernames and pulls them into a scam experience. B can also mark A's instanceClose, panicKill or shutdownSchedule as succeeded when A failed, or as failed when A succeeded, and show phishing text such as 'Session expired, re-pair at ...' as if it came from A. B was never paired with A, so this crosses the trust boundary between PCs.

**Fix:** Key pendingRpcs by `${pcId}:${reqId}` (or a Map per PC), and in the rpcResult handler accept a result only when p.pcId === rt.pc.pcId; otherwise drop it and count it as a protocol violation for that PC's session. Use a random 53-bit or per-PC reqId instead of one global counter. Check the shape of targetParse data before use (kind and field types), and show the resolved placeId and server details next to 'Ready to launch'.

### A27. Manual code is open to untargeted brute force across all users; the per-IP limit is bypassed by rotating IPv6 addresses (AAAA is live)
*low* (claimed medium) · `server/remote-relay/src/lib.rs:112` · area: crypto

**What:** The code space is 31^8 ≈ 2^39.6 per room (crypto.rs:113-127). The only throttle is 15 `pair_join` per 60 s per exact IP string (lib.rs:43-45, 112-125), keyed on X-Real-IP set by Caddy (lib.rs:148-157). roalt.dev publishes AAAA 2a02:4780:c:5355::1, and a commodity VPS gets a /64 (2^64 source addresses), so each new socket from a new v6 address gets a fresh bucket. There is no global cap. A guess matches any open room of any user (`manual_codes` lookup, lib.rs:644-649). The first joiner then locks the room (`busy`, lib.rs:666-669).

**Attack:** An attacker spreads guesses over thousands of v6 addresses. At 50k guesses/s with about 100 rooms open globally (thousands of users pairing), the expected time to a hit is about 2 days, and attempts never stop. On a hit, the attacker's join sets the PC's `pending` (mod.rs:1123). A legit phone that tries next gets 'busy', so the user has no phone SAS to compare against. The desktop still shows a pending card with an attacker-chosen label such as 'iPhone · Safari' and a SAS. Users who are mid-pairing tend to click Approve, which defaults to Control, and a random user's PC is taken over.

**Fix:** Bucket IPv6 by /64 (or /56) and add a global relay-wide `pair_join` budget with backoff. Raise code entropy, for example 10-12 characters. In the desktop UI, require the user to type the 6 digits shown on the phone instead of compare-and-click, so a join with no phone behind it cannot be approved. Show a warning when a join arrived while the user's own phone reported 'busy'.

### A28. Pairing blobs use the same key and AAD in both directions, so a relay can reflect the browser's join back as a valid 'ack'
*low* (claimed low) · `web/remote/src/transport.ts:476` · area: crypto

**What:** In QR mode the browser's join and the PC's ack are both `seal_pair_blob(qrKey, pairId, …)` with AAD = pairId (crypto.rs:329-351, crypto.ts:216-240), with no direction or type separation. `pairWithQr` decrypts any `{mode:"ack", blob}` and casts the plaintext to `PairAck` without validating it (transport.ts:476-487). The result is stored by `upsertPairedPc`.

**Attack:** The relay answers the browser's `pair_join` with `{mode:"ack", blob:<the browser's own sealed join>}`. It decrypts fine, and the browser saves a PairedPc whose pcId, pcPubB64 and roomToken are undefined. `RemoteManager.setPcs`/`sendSubscriptions`/`uuidToBytes` then misbehave or throw, and pairing never completes. The impact is availability and corrupted state, not confidentiality.

**Fix:** Use direction-specific AAD or info (for example `"roalt-pair-join"||pairId` vs `"roalt-pair-ack"||pairId`), or derive separate join and ack keys. Validate the ack strictly before storing it: pcId is a UUID, pcPubB64 decodes to a 65-byte P-256 point, roomToken is 43 base64url characters, and role is in the enum.

### A29. Handshake has no freshness or key confirmation: a replayed old handshake shows a device as connected on the desktop badge
*low* (claimed low) · `src-tauri/src/remote/mod.rs:720` · area: crypto

**What:** Each side signs only its own (role, pcId, deviceId, ephemPub) (crypto.rs:153-161). The signature does not cover the peer's ephemeral or any nonce, and no key-confirmation frame follows. When the PC verifies a viewer handshake, it immediately inserts into `connected_viewers`, updates last_seen and emits a change (mod.rs:722-741). The browser marks the PC 'online' when the PC handshake verifies (transport.ts:276-279).

**Attack:** A relay replays a handshake it recorded from a real past session of a paired device. The PC accepts it, shows 'Remote · 1' and 'connected' for that device, and starts streaming encrypted snapshots and previews that nobody can decrypt. Confidentiality holds because the session keys depend on the PC's fresh ephemeral. However, the security indicator (title-bar badge, device 'connected', last_seen) can be spoofed, and it costs CPU on the PC for capture and encode.

**Fix:** Count a viewer as connected, and start sending snapshots or previews, only after its first valid FRAME_DATA (key confirmation). Alternatively, have the viewer sign a transcript that includes the PC's fresh ephemeral (SIGMA-style).

### A30. Pairing brute-force rate limit can be bypassed over IPv6 and by drive-by browsers; there is no global budget
*low* (claimed medium) · `server/remote-relay/src/lib.rs:112` · area: relay

**What:** check_pair_rate keys on the exact X-Real-IP string (:152-158, :637), 15 attempts per 60 s. Caddy sets X-Real-IP to {remote_host}, and the relay port is bound to 127.0.0.1 (docker-compose.yml), so the header cannot be spoofed from the internet. However, roalt.dev serves IPv6 directly (AAAA 2a02:4780:c:5355::1, Server: Caddy, no CDN), so each address in an attacker's /64 gets its own bucket. There is no global limit on failed lookups. The rate-limited socket is not closed, and the WS handler does not check Origin, so any web page can make its visitors' browsers send pair_join guesses from their own IPs. The manual code space (31^8 ≈ 8.5e11, unbiased at crypto.rs:113-126) is fine against the intended 15/min, but against rotating IPv6 sources the only limit is relay throughput. Every concurrently open room across all users is a target, and a hit also makes the legitimate phone get 'busy'. On the other side, mobile carrier CGNAT puts many real users behind one IPv4, so a single abuser blocks pairing for all of them.

**Attack:** The attacker rotates source addresses across a /64 and sprays pair_join with random codes. A hit lands on some random user's open room, and the victim's desktop shows an Approve prompt at the moment they expect one. A victim who approves without comparing the SAS pairs the attacker's device.

**Fix:** Bucket IPv6 by /64 (or /56). Count only not_found results, and add a relay-wide not_found budget per second with exponential slowdown. Close the socket after rate_limited. Require Origin to be absent (desktop) or https://roalt.dev for browser connections. Consider 10-char codes.

### A31. Revoked devices can still check when the PC is online
*low* (claimed low) · `server/remote-relay/src/lib.rs:725` · area: relay

**What:** A Subscribe with a wrong or revoked token gets `pc_revoked` when the PC is online and `pc_offline` when it is not (:725-731). pc_hello also actively pushes `pc_revoked` to every subscriber with a non-matching token each time the PC connects (:476-477, :500-502). A revoked device, such as a lost phone or a former partner, keeps the pcId and can learn the PC's online times indefinitely.

**Attack:** A revoked browser keeps subscribing with its old token and logs pc_revoked and pc_offline transitions to learn when the victim's PC is on.

**Fix:** Send pc_revoked only once, on the actual revocation (the PcTokens path, :547-550), to attached viewers. After that, answer unauthorized subscriptions the same as offline, and do not push anything to unauthorized subscribers in pc_hello.

### A32. Docker hardening gaps
*low* (claimed low) · `server/remote-relay/docker-compose.yml:11` · area: relay

**What:** The good parts: non-root user 10001 (Dockerfile), read_only, no-new-privileges, port bound to 127.0.0.1. Missing: `cap_drop: [ALL]`, `pids_limit`, a `cpus` limit, an explicit `ulimits: nofile` (see the idle-timeout finding), and `tmpfs` is not needed. Both base images are unpinned (`rust:1-alpine`, `alpine:3.21` without a digest), so a rebuild can silently pull different toolchains or images. The relay trusts X-Real-IP from any peer that can reach it (lib.rs:152-158). That is fine behind Caddy, but any other container on the Docker bridge or any local process can spoof it.

**Attack:** A compromised sibling container or local process on the VPS connects to the relay directly and sets an arbitrary X-Real-IP to evade or poison the pairing rate limit. A future dependency-level exploit runs with default capabilities.

**Fix:** Add cap_drop: [ALL], pids_limit: 256, cpus: '1.0' and ulimits nofile 65536, and pin images by digest. Optionally accept X-Real-IP only when the TCP peer is the expected proxy address.

### A33. Snapshot overshares to every paired device, View-only included: free-form account notes, launch args, FFlags, profiles, and private-server link/access codes (even while the vault is locked)
*low* (claimed medium) · `src-tauri/src/remote/dispatch.rs:21` · area: dispatch

**What:** build_snapshot (dispatch.rs:21-57) receives the device role but uses it only as a label: every viewer gets the same data. The data includes full AccountSummary rows from db.accounts() (dispatch.rs:25, also broadcast in AccountsUpdated at mod.rs:916-921). Those rows carry `notes` (model.rs:75, free text up to 2000 chars, which account-manager users commonly fill with passwords, emails or 2FA backup codes), `launch_args` (model.rs:89), `custom_flags` (model.rs:100), `profile` and `client_limits`. Instances (dispatch.rs:33, InstancesUpdated at mod.rs:924) and bulk_progress (dispatch.rs:37) carry the decrypted LaunchTarget (model.rs:1341), which includes PrivateLink.link_code, PrivateAccess.access_code and ShareLink codes. These are the same values the code treats as vault secrets for favourites (vault-encrypted at commands.rs:3214, and withheld while the vault is locked at dispatch.rs:27-32), yet they are sent through instances and bulk regardless of lock state or role. Favourites with their link codes also go to Readonly devices. The dashboard reads none of these fields: grep of web/remote/src for notes, launchArgs, customFlags, linkCode, accessCode, profile and clientLimits finds nothing.

**Attack:** A user grants 'View' to a friend's or streamer's device, or to a shared or family tablet, expecting it to see status only. That device, or anyone who later gets its IndexedDB key (XSS on roalt.dev, a stolen phone), receives every account's notes, including any stored passwords, plus every private-server link or access code currently in use. With those codes they can join the user's private servers. Nothing in the UI tells the user that notes leave the PC.

**Fix:** Add a dedicated RemoteAccount DTO with only what the dashboard renders (id, userId, username, displayName, alias, color, favourite, sortOrder, groupIds, status, main) and never serialize AccountSummary over the wire. Do the same for Instance and BulkProgress: replace `target` with a redacted form (kind plus placeId, never link_code, access_code or share codes). Send favourites only to Control devices, as {id, name, placeId, accountIds}, and have the PC resolve the stored target by favourite id when a Control device launches one. Add a test that serializes a RemoteSnapshot built from fixtures with secret-looking values and asserts they are absent.

### A34. 'Disconnect all' (the title-bar panic pill) is not durable: any paired device re-attaches immediately by resubscribing
*low* (claimed medium) · `src-tauri/src/remote/mod.rs:905` · area: dispatch

**What:** ControlSignal::DisconnectAll (mod.rs:905-914) clears local sessions and sends kick_viewer, but the devices' room tokens stay valid. On the relay, KickViewer only removes the attachment and sends PcOffline (lib.rs:616-630). Subscribe re-attaches any viewer whose token hash is still allowed and emits ViewerJoined to the PC (lib.rs:713-722). The PC then handshakes and accepts the device again (mod.rs:664-687) with its full role. The dashboard also resubscribes on every socket reconnect (transport.ts:164-166, 183-201).

**Attack:** A user sees 'Remote · 1' and their clients moving, so they click the pill, which the UI presents as 'Disconnect all' with the toast 'Remote devices disconnected.'. A malicious or compromised Control device (a stolen phone, or XSS on roalt.dev) simply sends `subscribe` again and is back within milliseconds. Even a benign phone returns on its next network blip. The user believes access was cut when it was not.

**Fix:** Make Disconnect all a real pause: set an in-memory 'paused' flag (or set settings.remote.enabled=false) so the supervisor closes the relay socket and viewer_joined is refused until the user resumes in the desktop UI. Alternatively, rename it 'Disconnect for now' and offer 'Turn off Remote' and per-device revoke next to it. Revoke itself is correctly immediate (mod.rs:868-887).

### A35. Injected input is not confined to the target client: no hit test, the target area includes the window frame, and camera moves and right-button release ignore focus changes
*low* (claimed low) · `src-tauri/src/instances/remote_input.rs:326` · area: dispatch

**What:** point_at maps 0..1 onto DWMWA_EXTENDED_FRAME_BOUNDS (remote_input.rs:326-331), which is the whole window including the title bar and borders, not GetClientRect as the spec's Phase 2 note requires. Nothing checks that the screen point actually belongs to the client (no WindowFromPoint/GetAncestor(GA_ROOT) == hwnd). front() (lines 307-322) only checks that the client is the foreground window. InjectTouchInput, SetCursorPos plus SendInput(RIGHTDOWN) at line 213-215, and relative mouse moves all act on whatever window is topmost at that screen pixel: an always-on-top overlay, the taskbar under an edge-placed micro/tiled window, or a toast. Camera Move/Up (lines 219-234) and touch Move (186-193) never re-check the foreground. If focus changes mid-drag (the PC user clicks elsewhere, or anti-AFK's to_front cycle), relative mouse motion with the right button held goes into the other app, and the right-up lands there too (for example, an Explorer right-drag ends in its 'Move here/Copy here' menu). Keys are correctly limited to WASD/Space scan codes, so Alt+F4 and the Win key are not reachable.

**Attack:** Bad luck rather than attack. A phone tap on the lower edge of a tiled client that sits under the taskbar opens a taskbar item. A camera drag while a Discord or OBS overlay covers part of the client right-clicks the overlay. A drag in progress when the user clicks into their browser right-drags inside the browser.

**Fix:** Map coordinates onto the client rect (GetClientRect plus ClientToScreen) and match the capture to it. Before each Down (touch or camera), verify GetAncestor(WindowFromPoint(at), GA_ROOT) == hwnd and refuse otherwise. On every Move/Up, release the drag or finger if GetForegroundWindow() != hwnd instead of moving. Mark the instance as held (the existing 'Take control') while a phone is driving it, so anti-AFK, tiling and the timers leave it alone.

### A36. No per-device rate or concurrency limits on RPCs, snapshot requests, sessions or queued input
*low* (claimed low) · `src-tauri/src/remote/mod.rs:786` · area: dispatch

**What:** Every non-inline command is spawned with no bound (mod.rs:811-821). RequestSnapshot runs inline on the session loop and does synchronous DB reads, a vault decrypt per favourite and metrics sampling, and each reply can be about 256 KB (mod.rs:786-790). Read-only devices can do this. Each WebSocket a paired device opens becomes its own ViewerSession with its own handshake, snapshot and independent preview encoding of up to 16 targets at 4 fps (mod.rs:791-806, 1015-1073). Neither the relay nor the PC caps sessions per device. The input channel is an unbounded mpsc (remote_input.rs:105), and each touch Down on a non-foreground client blocks the worker for 300 ms or more (front()). A burst of events therefore builds a backlog, and ReleaseAll on disconnect is queued behind it, so stale taps keep stealing focus after the phone has left. Launches are already single-flight (bulk.rs:230-235) and batches one at a time, so mass launch is not the issue.

**Attack:** A View-only device, or a buggy dashboard retry loop, spams requestSnapshot or opens 20 tabs that each subscribe to 16 previews. The PC spends CPU on JPEG encoding and DB work and saturates its upload through the relay, which lowers FPS for the user's Roblox clients and adds relay bandwidth cost for everyone.

**Fix:** Add a small token bucket per ViewerSession (for example, 20 RPC/s, 1 snapshot per 2 s, input events 120/s) and answer 'rate_limited' when it is empty. Allow at most 2-3 concurrent sessions per device_id (kick the oldest). Cache the encoded JPEG per (instance, width, seq) across viewers. Make the input channel bounded (try_send, dropping Move events when full), and have release_all clear the queue (a generation counter) instead of queueing behind it.

### A37. Relay subscription cap of 16 PCs is silent, so PC 17 and later show 'Connecting…' forever
*low* (claimed low) · `server/remote-relay/src/lib.rs:693` · area: web

**What:** The dashboard subscribes all paired PCs on one socket (transport.ts:201-209) and has no limit. The relay keeps only the first MAX_SUBSCRIPTIONS = 16 (lib.rs:30, 693) and sends nothing back for the rest. The dashboard leaves those PCs in the 'connecting' state with no error (App.tsx:225-240).

**Attack:** Bad luck at scale: users with many rigs, or one browser shared across a team, pair 17 or more PCs. The newest PCs are listed first (storage.ts upsert puts new entries at the front), so older PCs silently stop connecting, and nothing in the UI explains why.

**Fix:** Have the relay send {t:'pc_err', code:'too_many'} for PCs beyond the cap, and make the dashboard enforce or show the cap. Alternatively, open a second socket per 16 PCs.

### A38. Manual-pair approval can be swapped to an attacker's request (pending silently overwritten)
*low* (claimed medium) · `src-tauri/src/remote/mod.rs:1123` · area: desktop

**What:** Each pair_joined that parses as a manual envelope replaces p.pending without checking for an existing one (mod.rs:1120-1131), so the most recent joiner always wins. PairDialog.decide (PairDialog.tsx:74-78) approves whatever session.pending.viewerId is current at click time. The dialog re-renders on every remote://changed, so the SAS digits and label can change in the instant before the click. pair_approve (mod.rs:334-350) only checks the viewer_id, never the SAS the user actually looked at. The label is attacker-controlled (up to 64 chars) and appears inside the prompt 'Check that this code matches the one shown on {name}'. Control is preselected (PairDialog.tsx:39).

**Attack:** An attacker who sees the 8-char code (on stream, over the shoulder, or through a clipboard sync from the Copy button) enters it right after the real user. The desktop now shows the attacker's SAS. A user who already compared the first SAS, or who clicks Approve as the digits change, approves the attacker's device with Control. Repeated joins can also keep overwriting a legitimate pending request, so pairing never completes.

**Fix:** Lock the pairing session to its first pending request. Reject later joins, or abort the session with a visible 'someone else tried to use this code, start again' message. Pass the displayed SAS to remote_pair_approve and compare it server-side with the stored pending.sas, refusing on mismatch. Disable Approve for about 1 s after pending changes, and keep the attacker-supplied label out of the verification sentence.

### A39. Any relay URL (including one restored from someone else's backup) receives the bearer credential that lets it squat this PC on the official relay
*low* (claimed medium) · `src-tauri/src/remote/mod.rs:622` · area: desktop

**What:** pc_hello sends pcSecretHash to whatever relay_url is configured (mod.rs:619-625). The relay authenticates PCs only by pcId == H(pcSecretHash) (relay lib.rs:446-465), so pcSecretHash is a static bearer credential, not a proof. validate_relay_url accepts any wss:// host (mod.rs:180-196). settings_set takes it with no confirmation (commands.rs:4112-4115). backup_export writes the full Settings, including remote.enabled and relayUrl (commands.rs:3570), and restore_settings applies them from a backup 'that may be anyone's' (commands.rs:3607-3610), so a shared backup or 'settings preset' quietly switches the relay. The pairing QR then carries ?relay= (mod.rs:292-297), which moves the phones there too.

**Attack:** A user who already has paired devices imports a backup from a 'RoALT settings pack' shared on Discord, or is told to use a 'faster relay'. RoALT reconnects at once to wss://evil and sends pcId and pcSecretHash. The attacker replays pc_hello on wss://roalt.dev/ws/remote. The relay accepts it because the hash matches, closes the real PC's socket (lib.rs:459-465), and with allowedTokens:[] sends pc_revoked to every phone of that user. The impostor and the real PC then knock each other off every few seconds, or the user's PC stays on evil while the impostor holds the official slot. The attacker's relay also logs the PC's IP, online times and viewer metadata. E2EE content stays safe because the static ECDSA key never leaves the PC.

**Fix:** Leave the remote section out of backup export and restore, or at least relayUrl and enabled. Ask for confirmation, with a warning, when the relay is changed away from the default. Never send the same credential to two relays: derive a per-relay secret, e.g. HKDF(pc_secret, relay origin). Better, replace the hash with challenge-response: the relay sends a nonce, the PC signs it with its static P-256 key, and pcId = H(static pubkey).

### A40. Synchronized reconnect storm after relay restart (no jitter on desktop, fixed 2.5 s on web)
*low* (claimed medium) · `src-tauri/src/remote/mod.rs:570` · area: desktop

**What:** After a connection that lasted more than 30 s, the backoff resets to exactly 2 s, then doubles 4/8/16/32/60 with no randomization (mod.rs:570-577). A relay or Docker restart drops every PC at the same instant, so every PC retries on the same schedule. Each retry is a full TLS handshake at Caddy, which answers 502 while the relay is down. The web dashboard reconnects every 2.5 s forever with no backoff and no jitter (web/remote/src/transport.ts:204). One relay serves all users, and each pc_hello walks every connected viewer (relay lib.rs:470-479).

**Attack:** An ordinary relay redeploy or a crash under load: thousands of desktops and every open phone dashboard hit Caddy in lockstep bursts at +2 s, +6 s, +14 s and so on, plus a flat 2.5 s pulse from browsers. The small VPS stalls during TLS and pc_hello processing, which makes connections fail again and keeps the herd synchronized. Relay availability suffers for all users.

**Fix:** Use full jitter on the desktop, e.g. sleep(rand(0..backoff)) with backoff capped at 60-120 s, and do not reset the backoff to the minimum on the first failure after a long session (start at rand(1..10 s)). Give the web the same exponential backoff with jitter, and pause reconnects while document.hidden. Optionally have the relay or Caddy answer 503 with Retry-After during startup and honour it.

### A41. 'Disconnect all' (title-bar pill) is undone automatically and by a page reload
*low* (claimed low) · `src-tauri/src/remote/mod.rs:905` · area: desktop

**What:** DisconnectAll only sends kick_viewer (mod.rs:905-914). The relay removes the attachment but keeps the viewer's subscription (lib.rs:616-630). On the PC's next pc_hello, after any network blip, settings change, relay restart or reconnect, every subscribed viewer with a valid token is re-attached and gets viewer_joined (lib.rs:470-499). A kicked phone can also simply reload and re-subscribe. The pill's tooltip, 'Click to disconnect all' (RemoteBadge.tsx:16-31), suggests a kill switch.

**Attack:** A user notices an unwanted session, for example the QR-on-stream attacker above, and clicks the pill expecting to cut it off. The attacker reloads and is back within seconds, or comes back silently at the next reconnect. The only real fix, Settings → Revoke, is not offered from the pill.

**Fix:** After Disconnect all, stop the kicked devices from rejoining until the user allows it: send pc_tokens without their hashes and keep that state until the user re-enables them or turns Remote off and on. Alternatively make the pill turn Remote off (enabled=false) and link to revoke, and label it accurately.

### A42. Relay connect has no timeout and the socket has no read deadline
*low* (claimed low) · `src-tauri/src/remote/mod.rs:615` · area: desktop

**What:** connect_async(relay_url) is awaited with no timeout (mod.rs:615), and ctrl_rx is not polled meanwhile. A TLS handshake to a server that accepted TCP but stalls (an overloaded Caddy during the storm above) can hang indefinitely. Disable, Reload and pairing signals queue up behind it, and turning Remote off still sends pc_hello once the connect completes. Once connected, the PC sends {"t":"ping"} every 25 s (mod.rs:936-937) but ignores the reply, so a half-open socket after sleep or a Wi-Fi switch looks Connected until a TCP write finally fails.

**Attack:** After a relay hiccup or laptop resume, RoALT shows 'Connected' or 'Connecting' indefinitely while phones can't reach it. A user who switches Remote off during the hang still sees one more connection and hello go out.

**Fix:** Wrap connect_async in tokio::time::timeout of about 15 s and race it against ctrl_rx so Disable aborts it. Record the time of the last inbound frame (the relay's pong) and return Err when nothing has arrived for about 60 s.

### A43. Decrypted plaintext of unparseable remote requests is written to the log
*low* (claimed low) · `src-tauri/src/remote/mod.rs:768` · area: desktop

**What:** On a RemoteRpcRequest parse error, up to 200 bytes of the decrypted request are logged at warn level (mod.rs:768). The web dashboard always runs the latest build while desktops lag behind on auto-update, so 'unknown op' or shape mismatches will be common. Requests can carry launch targets and private-server link codes, which RoALT itself treats as bearer secrets and encrypts at rest (storage favourites comment). Users routinely share log files in support channels.

**Attack:** A dashboard update adds or changes a field in Launch, InstanceMove or TargetParse. Older desktops log the plaintext of every such request, including private-server links, and those logs end up pasted in a public support channel.

**Fix:** Log only the error kind, the op name (if it can be extracted) and the payload length. Never log decrypted content.

### A44. More than 64 paired devices are silently 'revoked' by the relay; device rows grow without bound
*low* (claimed low) · `src-tauri/src/remote/mod.rs:591` · area: desktop

**What:** current_token_hashes sends one hash per stored device (ordered created_at DESC, storage/mod.rs:1033-1036). The relay keeps only the first MAX_TOKENS_PER_PC = 64 (lib.rs:451-454, 511-514), so the oldest devices get pc_revoked even though the user never revoked them. Nothing caps or prunes remote_devices. Every re-pair from a browser with cleared storage adds a row, and the panel lists them all.

**Attack:** A long-time user who has re-paired over many months ends up with older phones or PCs that appear revoked with no explanation, and a device list cluttered with stale entries that hides an unwanted one.

**Fix:** Refuse new pairings beyond 64 devices with a clear message, or prune devices never seen in N days. Show a stale marker for old devices in the panel.

### A45. Preview encoding is not shared across viewers and blocks the session loop
*low* (claimed low) · `src-tauri/src/remote/mod.rs:1044` · area: desktop

**What:** send_due_previews builds work items per viewer per target (the comment says 'unique', but they are not deduplicated across viewers) and awaits one spawn_blocking that captures and JPEG-encodes all of them. Only then does the select loop handle input, RPC results or control signals (mod.rs:1021-1060). With 16 targets at up to 4 fps per viewer (mod.rs:792-796) and several viewers or tabs, the same frame is encoded N times. Touch, key and revoke handling then waits behind the encode batch.

**Attack:** Two phones or a few tabs watching a 16-client grid on a low-end PC: the CPU cost multiplies, and taps and joystick input feel laggy or arrive in bursts. A revoke or Disconnect all is delayed until the batch finishes.

**Fix:** Cache the encoded JPEG per (instance, shot.seq, max_w) and reuse it for every viewer. Run encoding on a separate task that hands frames back through a channel, so the select loop never awaits it. Cap the total preview budget per PC, not per viewer.

### A46. Dashboard shares the https://roalt.dev origin with the main site; device keys and room tokens are usable by any script there, and the origin is about to be frozen into shipped binaries
*low* (claimed medium) · `web/remote/src/storage.ts:46` · area: release

**What:** The browser identity (a non-extractable P-256 keypair, which is still usable by any same-origin script) and the paired-PC list with roomToken and pcPubB64 are kept in IndexedDB `roalt-remote-v1` on the origin https://roalt.dev (storage.ts:4, 46-74). The live response for https://roalt.dev/remote/ uses the site-wide policy shared with the main site: CSP connect-src includes api.github.com, discord.com and the visitors worker, frame-src includes youtube-nocookie, with frame-ancestors 'self' and X-Frame-Options SAMEORIGIN. So every page on roalt.dev (/play share links, /go/*, app.json and future pages) runs with the same authority as the dashboard. The desktop hardcodes WEB_BASE_URL = "https://roalt.dev/remote/" (src-tauri/src/remote/mod.rs:43) for QR codes, and the default relay is wss://roalt.dev/ws/remote (model.rs:480). Once 1.3.x ships, moving the dashboard means a redirect kept forever and every browser pairing again, because IndexedDB is per origin.

**Attack:** Any XSS or injected same-origin script on any roalt.dev page, or a compromised deploy of the marketing site, can open IndexedDB, take each paired PC's roomToken, sign handshakes with the non-extractable device key, and connect as that device: Control role means launching, closing and steering every paired PC's clients, and also reading the snapshot data above. CSP script-src 'self' reduces the injection surface but does not separate the main site's code from the dashboard's.

**Fix:** Before the first desktop release, serve the dashboard and relay from their own origin (e.g. https://remote.roalt.dev/ and wss://remote.roalt.dev/ws), with a dashboard-only CSP: connect-src wss://remote.roalt.dev, frame-ancestors 'none', no third-party hosts. Point WEB_BASE_URL and the RemoteSettings default at it in the same change, and keep /remote/ on the main site as a 301.

### A47. No protocol/app version or minimum-version gate in the relay handshake; old desktop builds can't be refused or told to update, and mismatches fail silently
*low* (claimed medium) · `src-tauri/src/remote/mod.rs:619` · area: release

**What:** pc_hello carries only pcId, pcSecretHash and allowedTokens (mod.rs:619-624), and subscribe carries no version either (lib.rs:205-223). The binary header has a version byte, but the relay silently `continue`s on any other value (lib.rs:362), and so does the desktop (mod.rs:713). On the dashboard a handshake that fails because the crypto or labels changed only goes to console.error and the PC shows 'offline' (transport.ts:280-285). appVersion only appears inside the encrypted snapshot, after a successful handshake (protocol.rs:174). RoALT updates only when the user clicks Update now (README.md:309), so vulnerable or old builds stay in use for months, and the single deployed dashboard plus relay has to keep working with all of them.

**Attack:** A security bug is found in, say, 1.3.12's remote dispatch. Even after the fix ships, the relay operator cannot stop 1.3.12 PCs from accepting remote commands, because the relay cannot tell versions apart. In the other direction, a fix that changes the crypto (new HKDF info strings, a frame v2) leaves every non-updated PC showing as 'offline' on the dashboard with no explanation. Users see Remote as broken and start re-pairing in a loop.

**Fix:** Add `"proto": 1, "app": env!("CARGO_PKG_VERSION")` to pc_hello, and `proto` to subscribe and pair_join. Have the relay answer with `{"t":"err","code":"upgrade_required","min":"x.y.z"}` when a PC is below a configurable MIN_APP_VERSION env var, and reply with an error instead of silently dropping an unknown binary version. Show 'Update RoALT to use Remote' on the desktop, and show 'This PC runs an older RoALT' on the dashboard when a handshake fails. Do this before the first release, because builds shipped without the field can never be gated.

### A48. Release pipeline: a push to main auto-publishes to all users; web/** is not ignored, and neither the release nor CI runs the dashboard interop or relay tests
*low* (claimed low) · `.github/workflows/release.yml:17` · area: release

**What:** release.yml publishes a non-draft release on every push to main (lines 3-13, draft=false on push). paths-ignore covers docs/** and server/** (lines 23-24) but not web/**, so a dashboard-only fix commit bumps the patch version and offers a desktop update to every install. The release gates (lines 107, 110) and ci.yml run only src-tauri cargo test plus the desktop npm scripts. The Rust-to-WebCrypto interop test (web/remote/src/crypto.test.ts) and the relay tests (server/remote-relay) never run in any workflow, so a desktop change to RemoteEvent, RemoteCommand or the crypto can ship to all users while the deployed dashboard no longer matches it. The working tree also mixes Remote with an unrelated Hall of Fame feature (src/features/hallOfFame, App.tsx:10), and tracked files depend on untracked ones (main.rs `mod remote`, App.tsx import). A partial `git commit -a` would fail the release build, which is safe because nothing gets published, but it shows the change set is not split. CHANGELOG.md has no Remote section, so the notes would fall back to raw commit subjects.

**Attack:** Bad luck at scale: a one-line dashboard CSS fix pushed to main cuts RoALT 1.3.13 for thousands of users with no desktop change. Or a desktop serde rename in RemoteEvent passes CI and auto-publishes, and every updated PC's dashboard view breaks until someone redeploys web/remote by hand.

**Fix:** Add `web/**` to paths-ignore. Add a CI job (and a release gate) running `npm --prefix web/remote ci && npm --prefix web/remote test && npm --prefix web/remote run build` and `cargo test --locked --manifest-path server/remote-relay/Cargo.toml`. Commit Remote and Hall of Fame separately, each with its untracked files. Write a CHANGELOG section for the release that introduces Remote.

### A49. Privacy docs now false: README says the only RoALT server is the online counter and lists every host RoALT talks to, without the relay
*low* (claimed low) · `README.md:422` · area: release

**What:** README.md:62 says 'the only RoALT server is an online counter'. README.md:422 says RoALT 'Talks only to' a list of hosts in which roalt.dev appears only for the sponsor file, and README.md:437 says it never sends telemetry beyond the online count. PRODUCT.md (local, gitignored) says 'no RoALT server' and 'Fully offline except for Roblox, Discord and GitHub'. With Remote on, the desktop keeps a WebSocket open to wss://roalt.dev/ws/remote. The relay sees the PC's IP, a stable pcId, the SHA-256 secret hash it authenticates with, online and offline times, how many and which devices connect, and traffic volume (live 1-4 FPS previews). The content is E2EE, but this metadata is real data leaving the PC, held on a VPS (Caddy access logs, Docker json-file logs), and none of it is documented. The in-app text (en.json:1864) mentions only E2EE and cookies.

**Attack:** No attacker needed: users and reviewers who rely on the README's exhaustive host list find an undocumented connection to roalt.dev and treat it as tracking. That is a trust and legal-notice problem for a tool whose pitch is 'local-only'.

**Fix:** Before release, add wss://roalt.dev/ws/remote to the README host list ('only when Remote is turned on and a device is paired or pairing'). Add a Remote paragraph to Safety and privacy listing what the relay can and cannot see, and say whether Caddy or Docker logs keep IPs and for how long (ideally turn off access logs for /ws/remote). Update PRODUCT.md's constraints and the 'only RoALT server' sentence.

### A50. Custom relay URL feature can't work against the deployed dashboard: CSP blocks it, and the README proxy configs differ from production
*low* (claimed low) · `web/remote/src/storage.ts:86` · area: release

**What:** Desktop Settings lets the user set a custom wss:// relay (mod.rs:180-196), and the QR then carries `?relay=<url>` (mod.rs:292-296), which the dashboard accepts from any link (storage.ts:86-88). The live CSP on https://roalt.dev/remote/ has `connect-src 'self' wss://roalt.dev ...`, so the browser blocks every custom relay. The README Nginx snippet (README.md:39) restricts it the same way, while the README Caddy snippet (lines 58-70) sends no security headers at all, even though Caddy is what production runs. The documented deploy configs therefore match neither each other nor the live server.

**Attack:** A user who sets their own relay pairs and gets 'Could not reach relay' with no explanation. If someone later loosens connect-src to make this work, any phishing link `roalt.dev/remote/?relay=wss://evil` would point browsers at an attacker relay that logs pairing metadata.

**Fix:** Either remove the custom relay setting from the shipped UI, or keep it only for ws://localhost dev and document that the hosted dashboard works only with its own relay. Replace the README Caddy block with the config actually deployed, including a dashboard CSP header.

### A51. PC reconnect backoff has no jitter: every relay redeploy triggers a synchronized reconnect burst from all PCs
*low* (claimed low) · `src-tauri/src/remote/mod.rs:502` · area: release

**What:** Backoff starts at a fixed 2 s (mod.rs:502), resets to 2 s when the connection had lasted more than 30 s, and doubles up to 60 s with no random component (mod.rs:567-577). When the single relay restarts (any deploy, or an OOM as in the finding above), every online PC reconnects at exactly T+2 s, then retries in lockstep at 4, 8, 16 s, each with a TLS handshake through Caddy, a pc_hello and a pair_open.

**Attack:** Bad luck at scale: with thousands of PCs, each relay restart turns into a synchronized burst on the same VPS that hosts the website. Combined with the 128 MiB limit, the burst can knock the relay over again right after it restarts.

**Fix:** Add full jitter to the delay: sleep(rand(0..=backoff)), or backoff/2 + rand(0..backoff/2). Do the same for the dashboard's connectRelay delay.

### A52. A revoked device can still see whether the PC is online
*low* (claimed low) · `server/remote-relay/src/lib.rs:713` · area: abuse

**What:** A revoked device keeps the pcId. When it subscribes with its dead token, the relay answers pc_revoked if the PC is online (lib.rs:725-729) and pc_offline if it is not (lib.rs:730-731). PcHello also pushes pc_revoked to such subscribers each time the PC connects (lib.rs:476-478, 500-502).

**Attack:** (c) An ex-partner or other former device holder, after being revoked, keeps a socket subscribed and logs exactly when the victim's PC is on, for as long as they like.

**Fix:** Answer the same message for an offline PC and an unknown or invalid token (for example always pc_offline). Do not push pc_revoked from PcHello to subscribers with invalid tokens; drop their subscription instead.

### A53. View-only devices get free-text account notes and private-server codes
*low* (claimed low) · `src-tauri/src/remote/dispatch.rs:25` · area: abuse

**What:** build_snapshot sends the full AccountSummary to every role (dispatch.rs:25, 42-57). That includes notes (free text up to 2000 chars; model.rs:75), launch_args and custom_flags, and favourites with PrivateLink/PrivateAccess codes when the vault is unlocked (dispatch.rs:28-32). Users often keep emails, PINs or password hints in notes, which is close to the 'secrets never leave' promise.

**Attack:** A device paired as 'View only' (for example a friend's phone, or one obtained by the scam path) reads every alt's notes and private-server access codes.

**Fix:** Strip notes, launch_args, custom_flags and private access/link codes from snapshots for Readonly devices. Consider leaving notes out of remote snapshots entirely, or making it opt-in.

### A54. One malformed snapshot from any paired PC crashes the whole dashboard for every PC, and it happens again on each reload
*low* (claimed medium) · `web/remote/src/App.tsx:79` · area: gap:Web dashboard isolation between PCs: one paired PC acting against another PC in the same browser

**What:** handleEvent assigns PC-supplied JSON straight into runtime state without checking its shape: rt.snapshot = ev.data (transport.ts:318), and instances, accounts and metrics are copied the same way (transport.ts:323-340). App renders data from every online PC, not just the selected one: totalRunning runs rt.snapshot.instances.filter(...) for every online runtime (App.tsx:75-83), and each switcher pill runs rt?.snapshot?.instances.filter(...) (App.tsx:129). The optional chain stops before .filter, so instances: undefined or null still throws. main.tsx has no error boundary, so React 19 unmounts the entire root on this render error and the page goes blank. The JSON.parse try/catch on the inbound chain (transport.ts:188) does not help, because the bad value only fails later, during render.

**Attack:** Hostile PC B (or just a desktop build with a different snapshot shape, which can happen at scale with auto-update) sends {t:'snapshot', data:{pcName:'x', role:'control'}} or {t:'instancesUpdated', instances:null}. The dashboard goes blank for all PCs as soon as B comes online. After a reload, B reconnects and sends the same payload, and the page crashes again within a second. The 'Forget PC' button is part of the crashed UI, so the user cannot remove B. The only way out is to clear the site data, which deletes every pairing key and room token, so each PC has to be re-paired in person.

**Fix:** Check every RemoteEvent before applying it: arrays must be arrays, required fields must exist, and anything invalid is dropped and ends that PC's session. Render each PC's views and its pill inside a separate error boundary, and compute totalRunning defensively (Array.isArray). Then bad data from one PC can only break that PC's card.

### A55. Pairing ack fields are stored without validation, so one bad ack breaks the relay subscription for every PC, and an ack can claim and overwrite another PC's record
*low* (claimed medium) · `web/remote/src/transport.ts:535` · area: gap:Web dashboard isolation between PCs: one paired PC acting against another PC in the same browser

**What:** pcId, pcName, pcPubB64, roomToken and role come only from the PC's own decrypted ack (transport.ts:535-544 for QR, 629-638 for manual) and are not checked for type or format. upsertPairedPc (storage.ts:70-75) replaces any existing record with the same pcId: `[pc, ...list.filter(p => p.pcId !== pc.pcId)]`. It does not compare pcPubB64 and does not ask the user. App.handlePaired saves the result straight away (App.tsx:55-61). sendSubscriptions then puts every PC on that relay into one subscribe message (transport.ts:209-216). The relay parses it as Vec<SubItem> with typed pc_id: Uuid, device_id: Uuid and room_token: String fields (server/remote-relay/src/lib.rs:205-223). One bad item fails the whole message, and the relay replies bad_json and ignores it (lib.rs:350-356). The browser drops that reply because it has no pcId (transport.ts:226-227). I confirmed this with a throwaway test on a copy of the relay crate: a subscribe message holding one item with pcId 'not-a-uuid', a numeric roomToken, or no pcId is rejected even when a valid item is next to it. The relay ties a pcId to the PC's secret hash (lib.rs:286-293, 447), so a claimed pcId cannot be used to impersonate PC A on the relay, but the browser's stored record for A is still replaced.

**Attack:** (a) A scammer's PC B is paired through its QR code and sends an ack with pcId:'x' or roomToken:5. The record is saved to IndexedDB at the top of the list. From then on, every subscribe for that relay (on each reload and reconnect, and in practice that is all PCs on wss://roalt.dev) is rejected, so every PC shows 'Connecting…' forever with no error, and nothing points to the record at fault. (b) If B knows PC A's pcId (a former or revoked device of A, someone in the same household, or anyone who has the user's backup), its ack claims pcId A with B's pcPub, B's roomToken and A's pcName. A's real roomToken and pinned key are deleted. The relay answers the next subscribe with pc_revoked, and the UI says 'This browser was revoked from <A>'. The user loses A until they re-pair it in person.

**Fix:** Check the ack before saving: pcId must be a canonical lowercase UUID, pcPubB64 must decode to a 65-byte P-256 point, roomToken must be a string of the expected length, and role must be one of the known values. In upsertPairedPc, refuse to replace a record when the pcId already exists with a different pcPubB64, and require 'Forget' first or an explicit re-pair confirmation. Better still, make pcId derivable from pcPub (for example, the relay checks pcId = H(pcPub) or covers it with the secret hash) so the browser can verify the binding. On the relay, parse subscribe items one by one and skip invalid ones instead of rejecting the whole list.

### A56. A hostile PC can starve or freeze other PCs' sessions through the shared viewer queue, the shared inbound chain and unbounded preview state
*low* (claimed medium) · `server/remote-relay/src/lib.rs:383` · area: gap:Web dashboard isolation between PCs: one paired PC acting against another PC in the same browser

**What:** All PCs that one browser subscribes to share a single relay queue for that browser (OUTBOUND_CAP 64, CONTROL_RESERVE 8, lib.rs:36-38), and a PC's binary frame is silently dropped whenever capacity <= 8 (lib.rs:383-385). In the browser, every frame from every PC is handled one at a time on a single promise chain (transport.ts:184-188), and outbound seals share one chain too (transport.ts:461-462). handlePreview accepts any instanceId, not only subscribed targets. It adds a new key to rt.previews (copying the whole object each time) and a Blob to this.frames for each new id, calls emit(), which re-renders the whole app, and sends an ack back (transport.ts:379-397). This is the cross-PC side of the existing 'unmetered tunnel' finding: the issue here is the lack of fairness between PCs on one viewer.

**Attack:** Hostile PC B keeps the browser's relay queue full with 256 KiB data frames. PC A's handshake reply, snapshots and rpcResults are dropped at the relay. A dropped handshake reply leaves A at 'Connecting…' for good, because the browser only starts a handshake on pc_online (transport.ts:231-244) and has no timeout. A dropped rpcResult makes A's commands time out. Alternatively, B sends preview frames with a fresh instanceId and a large JPEG each time. rt.previews and this.frames grow without limit, each frame triggers a full re-render that copies an ever-larger object, and the tab freezes or runs out of memory, cutting off all PCs.

**Fix:** On the relay, give each (viewer, PC) pair its own budget or queue, so one PC cannot fill another PC's share. In the browser, process each PC's inbound frames on a separate chain. Drop preview frames whose instanceId is not among the targets last sent in setPreviewTargets, cap rt.previews and frames per PC, and close a PC's session if it sends unsolicited or oversized frames.

### A57. Launch and Clients view state carries over when switching PCs, so data entered for PC A is sent to PC B
*low* (claimed low) · `web/remote/src/App.tsx:261` · area: gap:Web dashboard isolation between PCs: one paired PC acting against another PC in the same browser

**What:** <ClientsView>, <LaunchView> and <MonitorView> are rendered without key={selectedPcId} (App.tsx:260-262), so React keeps the same component instance and its state when activeRt changes to another PC. LaunchView keeps linkInput, parsed (which can include a private-server linkCode or accessCode the user pasted for PC A) and selected (PC A's account ids) (LaunchView.tsx:16-18). The Launch button stays enabled and sends parsed.target with those ids to the new PC (LaunchView.tsx:241-242, 70-76). ClientsView keeps limitsInst and focusedId the same way (ClientsView.tsx:49-52).

**Attack:** The user pastes a private-server link on PC A's Launch tab, switches to PC B with the pill and taps Launch. The private-server link code (a credential the snapshot otherwise protects with the vault) and the chosen target are sent to B. A hostile B receives it, and an honest B launches whichever of its own accounts happen to have A's account ids.

**Fix:** Add key={activeRt.pc.pcId} to the three views, so all per-PC state resets when the PC changes, and drop stale targetParse results that come back after the PC changed.

### A58. The PC-side 160..640 preview width limit is not enforced. The remote and the desktop dashboard share one picture and one scaling setup per window, so each overrides the other's size and the GPU rebuilds full-window textures on almost every frame
*low* (claimed medium) · `src-tauri/src/instances/capture.rs:303` · area: gap:Capture/preview pipeline driven by View-only devices: which window gets captured, unsafe code, resource bounds

**What:** capture.rs keeps one Capture per HWND (captures map, :178), one scaled-texture set per capture (`scaled`, :139) and one picture per HWND (SHOTS, :62, written at :358). Each request overwrites `c.max_w` (:303). copy() rebuilds the textures whenever the mip level for the new max_w differs (:407-410), and that rebuild allocates a full window-size mip chain, an SRV and a staging texture (:453-461). The phone asks for 320 (grid, 15 fps) or 640 (focused, 30 fps) (web/remote/src/components/ClientsView.tsx:117,121). The desktop cards ask for 480 or 960 (src/features/dashboard/useThumbnail.ts:68), and the command accepts up to 1280 (src-tauri/src/commands.rs:2582). On a 1920-wide client that is mip level 3 (phone grid) against level 1 or 2 (desktop). Whenever both consumers are active, which on Windows 11 means borderless capture with the RoALT window visible, the level flips on nearly every request. grab_jpeg clamps the width it asks for (preview.rs:100) but then JPEG-encodes whatever SHOTS holds, at that picture's own size (preview.rs:116). Nothing on the PC checks shot.thumb.width against clamped_w, and nothing checks the JPEG's size before it is sealed and sent (mod.rs:1063-1067).

**Attack:** Bad luck at scale: a Win11 user leaves RoALT open on a second monitor while their phone shows the client grid. A View-only device can also cause it on purpose by subscribing at 320 to every client the desktop is showing. Each alternation allocates and frees an 11 MB texture (1920x1080 BGRA with mips) plus staging, up to roughly 35 times a second, on a GPU that is already rendering several Roblox clients. That causes stutter in the games. The desktop cards switch between 240-wide and 480/960-wide pictures, and the canvas is reallocated on every size change (useThumbnail.ts:76-77). The phone receives 960-wide JPEGs instead of 320/640, which is 2-9x the bytes and encode time, so the PC-side FPS budget in clamp_targets no longer bounds bandwidth. Because the JPEG size is never checked, any frame whose sealed size exceeds MAX_BINARY_FRAME+1024 makes the relay drop the PC's socket (server/remote-relay/src/lib.rs:159-160), and frames between 256 and 257 KiB are dropped silently (lib.rs:362).

**Fix:** Key the scaled textures and the stored picture by (hwnd, mip level) instead of by hwnd alone, or keep one Scaled per requested level and generate all requested levels from one CopySubresourceRegion+GenerateMips. In grab_jpeg, reject or downscale any shot whose width is greater than clamped_w. Cap the encoded JPEG (for example 200 KiB); above that, re-encode at a lower quality or send a status-only frame, so the relay frame cap can never be reached.

### A59. The PC shows no indication while a device is streaming live video: Windows 11 capture is borderless, and the only cue is an in-app pill that does not distinguish watching from connected
*low* (claimed low) · `src-tauri/src/instances/capture.rs:385` · area: gap:Capture/preview pipeline driven by View-only devices: which window gets captured, unsafe code, resource bounds

**What:** Remote preview turns off the WGC yellow border wherever Windows allows it (`session.SetIsBorderRequired(false)`, capture.rs:385, after `borderless()` at :248-256), so Windows 11 shows nothing on screen. A View-only device can start capture freely (SubscribePreviews is read-only, dispatch.rs:101, and is accepted at mod.rs:795-797). The only cue on the PC is RemoteBadge (src/features/remote/RemoteBadge.tsx:12). It renders only inside RoALT's own title bar, only while `activeViewers > 0`, and shows the same text whether the device is idle or streaming 30 fps of every client. src-tauri/src/remote/mod.rs has no tray, toast or OS notification. RemoteSettings (model.rs:463-472) has no per-device or global switch to turn previews off for View devices.

**Attack:** A device that gained access, for example through any of the already-known pairing weaknesses, can watch every Roblox window live (chat, Robux prompts, in-game UI) while RoALT sits minimized to the tray, and the user has no visible sign. On Windows 10 the yellow border is the only cue, and it appears around the game the user is playing without any prior opt-in setting.

**Fix:** Add a distinct 'watching' state: set it in RemoteState when any session has non-empty previews, and show it in the tray icon or tooltip as well as the title-bar pill, with a one-click 'stop previews'. Add a setting, or a per-device flag next to the role, that refuses SubscribePreviews for View devices (or for all devices).

### A60. Session handshake signatures do not cover the peer's ephemeral key, so old handshakes can be replayed
*info* (claimed low) · `web/remote/src/crypto.ts:77` · area: web

**What:** The handshake transcript is domain || role || pcId || deviceId || own_ephem (crypto.ts:77-79). Neither side's signature covers the other side's ephemeral key or any fresh value. onBinary accepts any validly signed PC handshake while sess.cipher is null (transport.ts:273-279).

**Attack:** The relay records one PC handshake frame for a given (pcId, deviceId) and can replay it to that browser on every later pc_online. The browser accepts it and derives keys from the stale PC ephemeral key. If that key's private half ever leaks (crash dump, memory disclosure), the relay can impersonate the PC to that browser indefinitely and feed it fake snapshots. Without the leak this is only a silent dead session, i.e. a DoS.

**Fix:** Use a SIGMA-style handshake. Whoever responds second signs a transcript that includes both ephemeral keys, or both sides exchange key-confirmation MACs over the full transcript (both ephemeral keys, both static public keys, pcId, deviceId) before marking the session online.

### A61. The dashboard sends one long-lived deviceId to the relay for every PC, letting the relay link a phone to all PCs it controls
*info* (claimed low) · `web/remote/src/transport.ts:206` · area: web

**What:** getOrCreateIdentity creates one deviceId and one signing key per browser (storage.ts:47-60). It is sent in plaintext in every subscribe message for every PC (transport.ts:206), and also to any PC that pairs via a #pair link.

**Attack:** Even an honest-but-curious relay can build a lasting graph of which PCs each phone controls, across all users. A malicious PC reached through a #pair link learns the same identifier that the victim's other PCs see. This weakens the 'relay is blind' and 'no central DB' story on metadata.

**Fix:** Generate a separate deviceId (and ideally a separate signing key) for each paired PC and store it in PairedPc, so identifiers cannot be linked across PCs.

### A62. The site-wide production CSP blocks the custom-relay ?relay= feature, so custom-relay QR links always fail
*info* (claimed info) · `web/remote/src/storage.ts:86` · area: web

**What:** The desktop app adds ?relay=<custom> to the QR URL when the user has set a custom relay (src-tauri/src/remote/mod.rs:291-297), and defaultRelayUrl accepts any wss:// value from that parameter (storage.ts:86-89). The CSP actually served on https://roalt.dev/remote/ allows only connect-src 'self' wss://roalt.dev, so any other relay is blocked and the user sees 'Could not reach relay'. On security this works in the product's favour: the CSP is currently what stops a phishing link like roalt.dev/remote/?relay=wss://evil from turning the first finding into a no-compromise attack. HSTS is also sent without includeSubDomains/preload.

**Attack:** No attack today. If the CSP is loosened to make custom relays work (connect-src wss:), any malicious link with ?relay=wss://attacker sends manual pairing through an attacker relay. Combined with the first finding, the attacker gets a Control device on the victim's PC with no infrastructure compromise.

**Fix:** Either drop ?relay= from the hosted dashboard (custom-relay users host their own build), or allow it only after an explicit confirmation screen showing the relay host. Never widen connect-src to wss: before the first finding is fixed. Add includeSubDomains to HSTS once all subdomains are HTTPS.

### A63. Dev toolchain dependencies have known advisories (vitest 3.2.x with tinypool and @vitest/mocker)
*info* (claimed info) · `web/remote/package.json:25` · area: web

**What:** npm audit --omit=dev reports 0 vulnerabilities, so the shipped bundle (react and react-dom only) is clean. A full npm audit reports tinypool <=2.1.1 (critical, prototype pollution leading to RCE: GHSA-5gmw-xhrv-c9v3, GHSA-85c8-ppgw-ccpr) and @vitest/mocker (moderate: GHSA-82fw-gwwq-j7x9, GHSA-qxg6-89mg-rqv8), both pulled in by vitest ^3.2.4.

**Attack:** Only affects developer or CI machines running the tests. Nothing reaches users' browsers.

**Fix:** Upgrade vitest to 5.x (npm audit fix --force) and re-run crypto.test.ts.

### A64. Role handling fails open to Control
*info* (claimed low) · `src-tauri/src/storage/mod.rs:1041` · area: desktop

**What:** Any role string other than 'readonly' maps to Control (storage/mod.rs:1041 and 1063). The column default is 'control' (020_remote.sql:7), remote_pair_approve defaults a missing role to Control (commands.rs:4193), and the QR path hardcodes Control. Any future role value, typo or corrupted row grants full control.

**Attack:** A later release adds a role such as 'viewer' or 'limited' and the user then downgrades RoALT, or a row gets corrupted. The device silently gets Control.

**Fix:** Parse 'control' explicitly and map everything else to Readonly. Default the column and remote_pair_approve to readonly.

### A65. Relay runtime image alpine:3.21 is past end of support; builder image is unpinned
*info* (claimed low) · `server/remote-relay/Dockerfile:8` · area: release

**What:** The runtime stage is `FROM alpine:3.21` (Dockerfile:8). Alpine 3.21's support ended 2026-11-01, so its busybox and musl get no more security fixes. The builder is the floating `rust:1-alpine` (Dockerfile:1), so a rebuild for an urgent fix can pull a different toolchain from the one that was tested.

**Attack:** Hardening only: a busybox or musl CVE in the internet-facing relay container stays unpatched. A rebuild during an incident breaks on a toolchain change.

**Fix:** The binary is static musl, so use `FROM scratch` (or gcr.io/distroless/static) with `USER 10001`, or at least a supported alpine pinned by digest. Pin the builder, e.g. `rust:1.xx-alpine3.23@sha256:...`.

### A66. web/remote dev dependency vitest 3.2.7 has open advisories (tinypool critical, @vitest/mocker moderate)
*info* (claimed info) · `web/remote/package.json:23` · area: release

**What:** `npm audit` in web/remote reports 3 vulnerabilities: tinypool <=2.1.1 (GHSA-5gmw-xhrv-c9v3, GHSA-85c8-ppgw-ccpr, prototype pollution to RCE) and @vitest/mocker <=4.1.11 (GHSA-82fw-gwwq-j7x9, GHSA-qxg6-89mg-rqv8), both pulled in by vitest ^3.2.4, which resolves to 3.2.7 and also installs a second copy of vite (7.3.8) next to vite 8.3.8. All of these are dev-only and not in the shipped dashboard bundle.

**Attack:** Developer-machine and CI exposure only; nothing reaches users.

**Fix:** Bump vitest to a fixed major compatible with vite 8 (4.1.11+ or 5.x) and re-run npm audit.

### A67. A paired PC can change its displayed name at any time to match another PC in the switcher
*info* (claimed low) · `web/remote/src/App.tsx:151` · area: gap:Web dashboard isolation between PCs: one paired PC acting against another PC in the same browser

**What:** Pills and headers show rt?.snapshot?.pcName || pc.pcName (App.tsx:151; also LaunchView and the vault banner at App.tsx:251). The snapshot value is sent live by the PC and takes priority over the name stored at pairing time. Nothing warns when two PCs show the same name, and the PC's identity (pcPub fingerprint) is never shown.

**Attack:** The user pairs a friend's PC B under the name 'Jake's PC'. Later, B's snapshot renames it to 'Gaming PC', the name of the user's own PC A, possibly with a matching instance list. The user then drives B believing it is A: launch selections, control input and pasted private-server links go to B.

**Fix:** Show the name stored at pairing time (pc.pcName) as the main label. If the snapshot name differs, show it only as a secondary label with a 'renamed' marker, and warn when two paired PCs have the same displayed name.

### A68. PID reuse window: a preview can capture a non-Roblox app's main window in the second after a client exits
*info* (claimed low) · `src-tauri/src/instances/tracker.rs:888` · area: gap:Capture/preview pipeline driven by View-only devices: which window gets captured, unsafe code, resource bounds

**What:** send_due_previews resolves the target with tracker.window_pid (mod.rs:1044). That function returns e.inst.pid while the cached `e.alive` flag from the last poll is true (tracker.rs:888-893), and polls run about once a second (tracker.rs:1172 comment). window::preview then calls process::main_window(pid) (window.rs:111). main_window picks the largest visible, unowned top-level window of whatever process now owns that PID (process.rs:283-318). It never checks the executable name or the start time, and the window cache refreshes as soon as the old Roblox HWNDs become invalid (process.rs:242-253). capture::request then starts WGC on that HWND (window.rs:115).

**Attack:** Plain bad luck: a client exits or crashes, and before the next tracker poll Windows hands its PID to a newly launched process with a visible window (a browser or password-manager window opening). A subscribed View device can receive one or a few frames of that app. The chance is low but not zero, given thousands of users and constant client restarts.

**Fix:** In the preview path, check the window's identity before requesting a capture. Have main_window, or a remote-only wrapper, confirm with GetWindowThreadProcessId plus process::start_time(pid) that the start time equals the entry's recorded start_time, or that exe_name is RobloxPlayerBeta.exe for unreadable clients. Return NoWindow otherwise. Alternatively, have window_pid return (pid, start_time) and check both.

