# RoALT Remote: threat model and residual risks

What RoALT Remote protects, from whom, and the risks that stay after the fixes in
[REMOTE_SECURITY_AUDIT.md](REMOTE_SECURITY_AUDIT.md). The wire and crypto details are in
[REMOTE_DASHBOARD.md](REMOTE_DASHBOARD.md).

## 1. The parts

| Part | Where | Holds |
|---|---|---|
| RoALT on the PC | the user's Windows PC | The PC's identity key, the list of paired devices and their roles, everything Remote shows. It approves every pairing and is the trust anchor. |
| Dashboard | `https://remote.roalt.dev/`, static files built from `web/remote/` | Each browser's identity key (non-extractable, in IndexedDB on that origin) and its list of paired PCs. |
| Relay | `wss://remote.roalt.dev/ws/remote`, built from `server/remote-relay/` | Nothing durable. It routes frames between PCs and browsers and keeps rooms and connections in memory only. |

Remote is off by default. RoALT opens no connection to the relay until the user turns Remote on, and
then only while a device is paired or a pairing is open.

## 2. What end-to-end encryption covers

- **Sessions.** Each session runs an ECDH P-256 handshake signed by the PC's and the device's
  identity keys, then AES-256-GCM in both directions. Snapshots, commands, results, previews and
  input all travel inside it. The relay forwards bytes it cannot read.
- **Pairing.** The QR code carries its secret in the URL fragment, which browsers never send to a
  server. A code pairing uses a commit-then-reveal exchange and a 6-digit check code over the whole
  transcript. The device's identity travels sealed and signed, and the PC's reply is signed by the
  PC's identity key. The PC accepts one joiner per pairing, and nothing is paired until the user
  clicks Approve on the PC and then confirms on the device (for a code pairing: that both screens
  show the same check code). Only then does the PC enroll the device.
- **Custom relays.** A relay other than RoALT's gets a credential derived for that relay alone, so
  it can't use what it learns to take the PC's place on RoALT's relay.
- **Never sent at all**, even to a Control device: `.ROBLOSECURITY` cookies, vault passwords,
  recovery codes, cookie exports, account notes, launch arguments, custom FFlags, performance
  profiles, and private-server link and access codes.

Not covered: the metadata in risk 3, and the dashboard's own code (risk 1).

## 3. Adversaries

| Who | Can | Cannot |
|---|---|---|
| Someone who reads the relay's traffic or the host's logs | Everything in risk 3 | Read any session, pairing or preview |
| A compromised relay process | Drop, delay or refuse frames, break a pairing or a session, see metadata | Read content, enroll its own key (it would need the user's Approve, and in a code pairing a 1-in-a-million match of the check code per attempt) |
| A stranger who sees the QR code or the pairing code within its 120 s | Send one pairing request that the PC shows to the user | Pair without Approve on the PC; match the check code |
| Whoever controls the server that serves the dashboard | See risk 1 | Pair a new device without Approve on the PC; get cookies or vault secrets |
| A paired View device | See clients, live previews, the monitor and crashes | Launch, close, change anything, or send input |
| A paired Control device | See risk 2 | Read cookies, vault secrets or the items listed in section 2 |
| A hostile PC paired to the same dashboard | Send its own events to that dashboard | Answer another PC's requests (request ids are random and kept per PC) |
| Other scripts on `roalt.dev` | Nothing: the dashboard has its own origin | Reach the dashboard's keys or storage |

## 4. Residual risks

These are known and accepted. They are documented here, in the README, and in the app's pairing
copy.

### 4.1 Whoever controls the server can serve malicious dashboard code

The dashboard's files and the relay run on the same VPS behind the same web server. Against a
passive operator the relay is blind, but someone with root on that server can serve a modified
dashboard. That code runs with every returning browser's stored identity key, so it can act on
every PC that browser is paired with, with that device's role. It can also read a QR code's
`#pair=` secret when the link is opened. Subresource Integrity cannot help, because the HTML that
would carry the hashes comes from the same server.

What still holds: a new device needs Approve on the PC, every new pairing shows a notice there, the
title-bar pill shows which devices are connected, and cookies and vault secrets are never sent.
Revoke a device, or turn Remote off, and that key is worthless. Hosting the static dashboard on
separate, CI-built infrastructure would split relay compromise from code compromise; until then this
risk stays.

### 4.2 Control is account-level power

Cookies never leave the PC, but a Control device does not need them. It can launch any account into
any place or private server, stop every client, schedule a shutdown, and tap and press keys inside a
client, including purchase and trade screens. That can move Robux and items. New devices start as
View, and the pairing copy says what Control can do. Give Control only to your own devices, and never
show the QR code or the pairing code on a stream, in a screen share, or to anyone who offers help.

### 4.3 The relay sees metadata

Content is encrypted, routing information is not. The relay, and anyone who reads its traffic or the
host's logs, can see:

- the IP address of the PC and of each browser, and each browser's user agent and origin;
- the PC's stable pcId and the hash it authenticates with, its RoALT version, and the hashes of its
  devices' room tokens;
- when the PC is online, and when each device connects;
- each paired device's id, how many devices a PC has connected, and which PCs one browser follows
  (one browser connection carries all of its PCs);
- that a pairing is open, its pairId and pairing code, and when it was joined;
- the size and timing of every frame, which shows activity such as a live preview or input.

It cannot see PC or account names, games, clients, previews, commands, check codes, QR secrets or
cookies. It keeps no database, and a restart forgets every room and connection.

**Logs.** At its shipped level (`RUST_LOG=info`) the relay program writes only its start and
shutdown lines and rare warnings: no addresses, pcIds or device ids. Docker keeps those logs rotated
at 3 × 10 MB (`server/remote-relay/docker-compose.yml`). The web server in front of it (Caddy, the
block in `server/remote-relay/README.md`) writes no access log for relay connections
(`/ws/remote`); for the dashboard's static files it keeps an access log with client IP addresses
and user agents, rolled at 10 MB; up to 3 rolled files are kept and each is deleted 72 hours after it
rolled, so on a quiet site the current file can hold older lines until it fills. The QR link never
names a relay, so a custom relay's address never reaches this log.

### 4.4 Version skew and the support window

RoALT updates only when the user clicks Update, so one hosted dashboard and relay serve a long tail of
desktop builds. Every PC and browser states its protocol version (`proto`, now 2) when it connects,
and the PC also sends its RoALT version. The relay refuses anything below its minimum with
`upgrade_required`; the PC then tells the user to update RoALT, and the dashboard asks to reload.

Support window: a change that breaks the wire format raises `proto`. The hosted dashboard and relay
keep accepting the previous `proto` for at least 90 days after the RoALT release that introduces the
new one, then raise the minimum. A security bug in an old protocol raises the minimum at once, with no
window. The dashboard and relay are deployed apart from the app: a push that changes only `web/` or
`server/` publishes no RoALT release.

### 4.5 Pairing codes can be guessed, harmlessly

A pairing code is 8 characters from a 31-letter alphabet, 31^8 ≈ 2^39.6, and lives for 120 s. The
relay limits pairing attempts per address, wrong manual codes to 1/16 of 600 a minute per IPv4 /24 or
IPv6 /32, and relay-wide to 600 a minute, after which it refuses manual-code joins from every network
that guessed wrong that minute. A network with no wrong code is still let in, so guessers can't switch
manual pairing off for others; a guesser spread over many networks gets 600 codes a minute plus one
per network that hadn't guessed yet, and could in theory hit someone's open code. A hit only puts a request on that PC: its check code will not match the
user's phone, it still needs Approve, and the PC then refuses the user's own join, so the worst case
is one pairing to start again. Accepted.

A new device stays pending on the PC until its first session decrypts, and a pending device is
deleted after 10 minutes of the PC being connected, awake and unpaused without that session. The
row keeps the time it has left, so neither reconnecting nor sleeping gives it more; only the browser
that paired holds its key, and the row shows as "Waiting for its first connection" until it expires
or the user revokes it. Pairing is refused while Remote is paused, and pausing closes an open
pairing. A browser that re-paired keeps both devices and proves both to the PC, which deletes
whichever of them it still has once the new device connects. Until one of the two has a session,
the browser switches between them every 15 minutes, never drops either, and refuses another
re-pairing of that PC.

### 4.6 Windows 10 shows a yellow border, Windows 11 shows nothing

While a device watches a client's live preview, Windows 10 draws a yellow border around that client.
RoALT cannot remove it, and it is the only cue from Windows itself. Windows 11 captures without a
border, so there the cues are RoALT's own: the Remote pill in the title bar and the notice for each
new pairing.

## 5. What users should do

- Pair only your own devices, and Deny any request you didn't start.
- Never show the QR code or the pairing code to anyone, on a stream or in a screen share.
- Keep devices on View unless they need Control.
- Revoke a lost or replaced device in Settings › Remote, and turn Remote off when you don't use it.
