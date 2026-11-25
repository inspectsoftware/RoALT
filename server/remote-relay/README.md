# RoALT Remote Relay (`server/remote-relay`)

Stateless, in-memory, end-to-end encrypted WebSocket relay for RoALT's Remote Dashboard. Production: dashboard at `https://remote.roalt.dev/`, relay at `wss://remote.roalt.dev/ws/remote`.

- **Zero persistence:** Stores nothing on disk and has no database.
- **Routes ciphertext only:** Forwards opaque E2EE frames between paired RoALT PCs and browser viewers. It never sees cookies, account lists, screenshots or keys.
- **What it does see (metadata):** client IPs (from Caddy's `X-Real-IP`), each PC's stable `pcId` and RoALT version (`pc_hello.app`), when PCs are online, how many devices attach, which pcIds a browser subscribes to, traffic volume, and pairing-room ids and manual codes while a pairing is open (120 s). Container logs (warnings and errors, no per-connection lines at the default `RUST_LOG=info`) are rotated at 3 × 10 MB. Caddy writes no access log for `/ws/remote`. The access log of the static dashboard (client IPs, user agents) rolls at 10 MiB; at most 3 rolled files are kept, each deleted 72 h after it rolled, so on a quiet site the current file can hold older lines until it fills. QR links never name a relay.

## Run Locally

```bash
cd server/remote-relay
cargo run
# Listens on 0.0.0.0:8788 (/ws/remote and /healthz)
cargo test
```

## Wire rules the relay enforces

| Rule | Behaviour |
|---|---|
| Protocol version | `pc_hello`, `subscribe` and `pair_join` must carry `"proto": 2` (`MIN_PROTO`). Missing or lower: `{"t":"err","code":"upgrade_required"}`, then close. |
| Binary version | A routed frame whose first byte is not `0x01`: `{"t":"err","code":"bad_version"}`, frame dropped. |
| Registration | The first `pc_hello` / `subscribe` / `pair_join` must arrive within 10 s. Binary before registration closes the socket. A viewer attached to no PC, waiting for no online PC that accepts its token, and holding no pairing room, and a PC that accepts no device token and holds no pairing room, get `{"t":"err","code":"no_purpose"}` and are closed after 30 s of that, however many pcIds a viewer subscribes to (a dashboard whose PCs are all offline reconnects later, with a growing backoff; RoALT only connects while a device is paired or a pairing is open). Attaching and detaching wake the socket, so the 30 s count from losing the last purpose; of a client's own messages only `pc_tokens`, `pair_open`, `pair_join` and `subscribe` (when the rate limit lets them through), a `pc_hello` that registers the socket, a PC's `pair_close` of its own room and an `unsubscribe` that ends a subscription make the relay check it again. Anything else that ends a socket's purpose (its PC leaving or registering again without its token, the PC revoking the token it waits with, its pairing room expiring) is noticed at the relay's next ping, so there the 30 s start up to 25 s late. This only clears sockets left with nothing to do: anyone can register a PC that accepts its own token and attach to it, so the connection caps, not this rule, bound a deliberate squatter. |
| App version | Optional `RELAY_MIN_APP_VERSION`: a `pc_hello` whose `app` is older gets `{"t":"err","code":"upgrade_required","min":"…"}`, then close. |
| Liveness | The relay sends a WS Ping every 25 s and closes any socket with no inbound text or binary message for 70 s (Ping/Pong frames don't count, so an unfinished fragmented message can't keep a socket open). A socket whose writes stall for 10 s, or whose own oldest queued message (a reply or control frame) waited 15 s, is closed; a peer's frame that waited 15 s is dropped instead. |
| Frame size | PC binary ≤ 256 KiB, viewer binary ≤ 64 KiB, text ≤ 32 KiB, and the `data` of a `pair_join` or `pair_msg` ≤ 8 KiB (`MAX_PAIR_DATA`). Larger: `{"t":"err","code":"frame_too_large"}`, frame dropped, socket kept. WebSocket frames or messages over 257 KiB end the socket. |
| Inbound rate | Token buckets per connection (PC: 4 MiB/s, 8 MiB burst, 400 msg/s; viewer: 256 KiB/s, 1 MiB burst, 200 msg/s), per client IP or IPv6 /64 across its sockets (8 MiB/s, 16 MiB burst, 1000 msg/s), and 2×, 4× and 8× that per IPv6 /56, site (IPv4 /24 or IPv6 /48) and IPv6 /32. A coarser group's budget only holds up a subgroup (the next finer prefix) that has been sending more than the group's rate split over its subgroups with a socket open (measured over about a second); a lighter one is still counted against it but never waits on it, so a few heavy /64s can't stall a light neighbour. Over budget the relay stops reading the socket until the budget refills, so TCP slows the sender down, and sends `{"t":"err","code":"rate_limited"}` (at most 1/s). `pc_tokens`, `pair_open`, repeated `subscribe` and a joiner's rejoin of its own room are limited to 4/s (8 burst) per socket; over that they are dropped with `rate_limited`. `unsubscribe` is not, so a dashboard leaving many PCs at once loses none; one for a pcId the socket doesn't subscribe to does nothing. A PC's `pair_msg` and `pair_extend` for its own room are not, so a burst of `pc_tokens` can't swallow a pairing's ack or `enrolled`. |
| Misrouting | Binary frames for a target the socket isn't attached to are dropped. A viewer socket sending more than 64 in 10 s is closed; a PC socket never is (viewers attach and leave at will). |
| Outbound queues | Byte budgets, charged to the receiving socket: 2 MiB per socket and 256 MiB for the whole relay. A message counts its length plus 256 bytes (what the relay holds per queued message), and leaves both budgets when the socket's writer takes it to send, so a slow reader holds at most that one message outside them (see the memory sizing) and nothing being written can pin the relay's budget. A routed frame reserves its socket's budget first: a full socket frees room from its own largest other peer stream if that holds more than the pushing peer's stream plus the frame, so a few busy PCs can't starve a dashboard's other PCs; otherwise the frame is dropped and nobody else is touched. It then reserves the global budget: a full relay frees room from a socket whose tenant holds at least what the receiver's holds, compared at the first address level where the two differ (IPv4 /24 or IPv6 /32, then the IPv4 address or the IPv6 /48, /56 and /64, then the sockets themselves), and picks the largest coarse to fine: an attacker split over many small addresses or sockets in one prefix is weighed as that prefix, a carrier's customers compete with each other per /48 and /64, and a neighbour of a prefix filling the relay takes room from that prefix, not the other way round. The victim loses the oldest frames of its largest stream (then the next), a few times what the push needs and at most 256 KiB; the relay's `pc_online` / `viewer_left` style messages and its other streams stay; a socket with no queued frames is never picked or closed. When one victim holds less than the frame needs, the next outranking ones shed too, up to 8, so a prefix filling the relay in many small pieces can't starve bigger frames; if those 8 together hold less than the frame, nobody sheds and the frame is dropped. A tenant no smaller than every other where they differ sheds nobody, so a prefix filling the relay only ever loses its own frames. Address groups have no budgets of their own, they only rank. The ranking is rebuilt at most every 5 ms for the whole relay and holds only the sockets that have queued frames when it is built (a push that finds too little to shed rebuilds it once more if a backlog has started since, so no frame is dropped for a stale ranking), and a push skips the entries that have nothing left to shed and sheds from at most 8, so queue pressure never turns into a sort per frame, and idle connections are never sorted. Frames still over budget are dropped. The relay's own messages (`pc_online`, `pc_offline`, `pc_err full`, `viewer_joined`, `viewer_left`, `pair_*`, pings, close) and the replies a client provokes (`pong`, errors) use a 64 KiB allowance per socket that is outside the routed budgets (counted apart as `controlBytes` in `/healthz`), so a flooder only ever fills its own, and a slow reader that keeps asking for replies holds none of the routed budgets; a reply that doesn't fit is dropped. Pairing payloads a client sent (`pair_joined`, `pair_msg`) use that allowance too and also count against the receiving socket's budget and its address groups' rank. Each socket's queue keeps one stream per peer (per PC for a viewer, per viewer for a PC), sent in turn after the socket's own replies, so one peer's backlog delays another's frames by at most one message. Relay messages about a peer (`pc_online`, `pc_offline`, `viewer_joined`, `viewer_left`) go in that peer's stream, in order with its frames. |
| Subscriptions | ≤ 16 PCs per viewer socket; each extra PC gets `{"t":"pc_err","pcId":"…","code":"too_many"}` (at most 64 such replies per `subscribe`). Invalid items are skipped one by one. |
| Viewers per PC | ≤ 16 attached at once, ≤ 3 per device token (a fourth socket with the same token replaces the oldest, which gets `pc_err` `replaced`). Extra subscribers with an accepted token get `{"t":"pc_err","pcId":"…","code":"full"}`, stay open, and are attached when a slot frees up. `viewer_joined` carries the `tokenHash` the viewer used. A viewer the PC kicked, or one a newer socket of its device replaced, is not attached again until it subscribes with `"restart": true` or another token, or the PC sends a new `pc_hello`; a plain `subscribe` (one sent for the dashboard's other PCs) only gets `pc_offline` for it. `kick_viewer` may carry `"code": "paused"` or `"device_sessions"`, which reaches the viewer as `{"t":"pc_err","pcId":"…","code":"…"}` instead of `pc_offline`. The dashboard stops retrying on `paused` (until `pc_online`) and, after one `restart` retry, on a second `replaced` within 10 minutes, so neither a paused PC nor a fourth tab makes it loop. Every subscriber waits on its pcId with no per-PC limit (the 16 subscriptions per socket and the connection caps bound it), so subscribers with bad tokens can't keep a real device from hearing `pc_online`; one whose token the PC has refused for 120 s, counted from the first refusal and across repeated `subscribe`s for that pcId whatever token they carry, even when one in between left the pcId out, is dropped from the wait list. A socket remembers its first refusal for up to 64 pcIds until that PC accepts it; a socket refused by more PCs than that doesn't wait on the others at all. When the PC connects, and whenever slots free up in any way (leave, kick, unsubscribe, or `pc_tokens` revoking an attached device or accepting new ones), they go to the subscribers still waiting, the longest waiting first, until the PC is full again (one that replaces a socket of its own device fills no slot, so the next in line still gets one). A `subscribe` item with `"restart": true` for a PC the socket is already attached to gets `pc_online` again and the PC gets `viewer_joined` again, so a dashboard that lost the PC's handshake starts a fresh session without giving up its slot. |
| Revoked vs offline | A wrong/revoked token and an offline PC both get `pc_offline`; revoked subscribers get nothing when the PC comes online. |
| Pairing rooms | A pairId is 16–64 hex characters, so it can never also read as a manual code (others get `pair_err bad_pair_id`). `pair_open` for a pairId or manual code that another PC holds: `{"t":"err","code":"pair_taken","pairId":"…"}`. The first joiner owns the room; when its socket closes the PC gets `{"t":"pair_left","pairId":"…","viewerId":"…"}`. A later joiner from another socket gets `pair_err busy`, and the PC still receives its `pair_joined` (with a different `viewerId`) so it can abort. `pair_msg` only reaches the first joiner, who may join its room at most 4 times (`pair_err busy` after). Once its PC approved the joiner, `{"t":"pair_extend","pairId"}` from the room's PC connection keeps the room for 60 s more (`PAIR_CONFIRM`), at most until 180 s after it opened, so the device has time to confirm; it does nothing for a room nobody joined. When a room with a joiner goes away (`pair_close`, the PC's socket closing, expiry), the joiner gets `pair_err closed`. Pair payloads are never inspected. |
| Pairing guesses | 30 pairings per minute per client IP (IPv6 bucketed by /64) and 60 per IPv6 /48: every `pair_join` counts except one from the socket that already holds that room (the manual flow's second and third join, the QR flow's second), so misses and contended joins still count. Over that, `pair_err rate_limited`, then close. Wrong manual codes: 600 per minute relay-wide (`MANUAL_MISS_MAX`), of which one IPv4 /24 or IPv6 /32 may spend 1/16 (`MANUAL_MISS_SHARE`). A network over its share, and once the whole budget is spent any network that guessed wrong this minute, gets `pair_err rate_limited` on every new manual-code join, right or wrong, then close, until the minute is over. A network with no wrong code this minute is always let in, so guessers can't switch manual pairing off for anyone else, and the relay-wide budget grows by at most one guess per network that hadn't guessed yet. QR joins (pairIds, 16+ random hex characters) are unaffected. |
| Connections | 4096 total by default (`RELAY_MAX_CONNS`). Nested caps: 32 per IPv4 address or IPv6 /64 (`RELAY_MAX_CONNS_PER_IP`; a home's 16 PCs and its dashboards share one public address); per IPv6 /56, site (IPv4 /24 or IPv6 /48) and IPv6 /32, 1/64, 1/16 and 1/4 of `RELAY_MAX_CONNS` (64, 256 and 1024 by default), so a carrier prefix holds hundreds of honest users. When an address is full, a newcomer from it closes that address's socket that has sent nothing for longest, if that is more than a ping interval and a fifth (30 s; live PCs and dashboards send a ping every 25 s), so after a router blip a home gets back in before the relay times its dead connections out; otherwise HTTP 429. When a group (or the relay) is full, a newcomer whose subgroup holds at least two fewer sockets than the largest subgroup there, compared at the first level where they differ (IPv4 /24 or IPv6 /32, then /48, /56, /64), closes the newest socket of that largest subgroup, however young, and takes its seat at once. The newcomer's subgroup at the level where they differ closes no other socket for 10 s, so churn from one subgroup trims at most once per interval; only the closing side waits, so a tenant that trims its own big subgroups can't shield them from a neighbour and shut the group, and the largest subgroup never trims anyone. Otherwise HTTP 429 (a full group) or 503 (a full relay). Admission checks and charges every level in one step, so concurrent upgrades can't overshoot a cap or a budget. New connections: 10 a second (20 burst) per address or /64; a coarser group gets that times its cap over the per-address cap. Every connection that passes them is charged to every level, but a coarser group's bucket only holds up a subgroup that has been connecting more than its share there (the group's rate split over its subgroups with a socket open, measured over about a second); subgroups the relay has not seen in the last 60 s count together as one. So rotating through fresh /64s or /48s is limited by the prefix above them, and neighbours that retry hard only use up their own share: a known neighbour, or one the relay has not seen, still gets in. An attempt refused by a connect budget costs nothing and leaves nothing behind; one refused after looking for a socket to close has spent its tokens. A group's buckets outlive its last socket by 60 s, so reconnecting doesn't refill them. The relay keeps at most 16 address groups per allowed connection; when that fills, it drops the idle ones (at most once a second) and otherwise answers HTTP 503. |
| Origin | Browser sockets must send `Origin: https://remote.roalt.dev` or `http(s)://localhost[:port]` / `http(s)://127.0.0.1[:port]` (dev). Other origins get HTTP 403. Sockets without an Origin (the desktop app) are allowed. |

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `RELAY_BIND` | `0.0.0.0:8788` | Listen address. |
| `RELAY_MAX_CONNS` | `4096` | Global connection cap. |
| `RELAY_MAX_CONNS_PER_IP` | `32` | Concurrent connections per IPv4 address or IPv6 /64. |
| `RELAY_MIN_APP_VERSION` | empty | Lowest RoALT version (`major.minor.patch`) a PC may connect with. |
| `RELAY_TRUSTED_PROXIES` | empty | Comma-separated peer IPs whose `X-Real-IP` is trusted, in addition to loopback. From any other peer the header is ignored and the TCP peer address is used. |

**Memory sizing.** Per connection, the worst case is the one message its writer is sending (at most 257 KiB, already out of the queue budgets) plus a 257 KiB frame being read, plus kernel socket buffers. Compose caps those at 256 KiB each way (`net.ipv4.tcp_rmem` / `tcp_wmem` in `sysctls`; socket memory counts against the container), so about 1 MiB per connection. The outbound budget (256 MiB in all) counts each queued message at its length plus 256 bytes, which covers what the relay keeps per message, so even a queue of tiny frames stays inside it; shedding never raises it, it only decides whose bytes go first. The relay's own messages and the replies a client provoked add at most 64 KiB per socket. Address groups take about 300 bytes each, at most 16 per allowed connection (about 15 MiB at 3072). With compose's `RELAY_MAX_CONNS=3072`: 3072 × (1 MiB + 64 KiB) + 256 MiB + 15 MiB ≈ 3.5 GiB, plus about 200 MiB of per-connection overhead, hence `mem_limit: 4g`. Queue pressure costs at most one ranking of the sockets holding queued frames every 5 ms; admission searches are paid for out of the newcomer's own connect budget. Every PC with Remote on and a device paired holds one connection around the clock, so a relay of 3072 serves about 2500 such PCs plus their open dashboards; run more relays (or raise both limits) before that. Raise `RELAY_MAX_CONNS` and `mem_limit` together, and keep `ulimits.nofile` above `RELAY_MAX_CONNS`.

## Deploy on the VPS (Docker)

```bash
cd server/remote-relay
docker compose up -d --build
curl http://127.0.0.1:8788/healthz
```

The image is built from a digest-pinned `rust:1.100.0-alpine3.24` and runs as a static binary in a `scratch` image as UID 10001, with `read_only`, `cap_drop: [ALL]`, `no-new-privileges`, `pids_limit`, `cpus`, `mem_limit` and `nofile` limits. To update the toolchain, bump the tag and digest together (`docker buildx imagetools inspect rust:<tag>`).

**Trusted proxy.** Caddy on the host reaches the container through the published `127.0.0.1:8788` port, so the relay sees the connection coming from the compose network's gateway, `172.31.88.1`. That is why compose sets `RELAY_TRUSTED_PROXIES=172.31.88.1`. If the subnet collides with an existing Docker network, change `subnet`, `gateway` and `RELAY_TRUSTED_PROXIES` together. If the relay logs `ignoring X-Real-IP from an untrusted peer`, set `RELAY_TRUSTED_PROXIES` to the peer address it prints. Otherwise every client shares one IP bucket and hits the per-IP cap.

## Reverse proxy (Caddy, `remote.roalt.dev`)

The dashboard has its own origin with no analytics or other content. `web/remote` builds with Vite `base: "/"`, and its `dist/` is copied to `/opt/roalt-remote/web`.

```caddy
remote.roalt.dev {
	encode zstd gzip

	handle /ws/remote {
		log_skip
		reverse_proxy 127.0.0.1:8788 {
			header_up X-Real-IP {remote_host}
		}
	}

	handle {
		root * /opt/roalt-remote/web
		try_files {path} /index.html
		file_server
	}

	header {
		Content-Security-Policy "default-src 'self'; connect-src 'self' wss://remote.roalt.dev; img-src 'self' blob: data:; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
		X-Frame-Options "DENY"
		X-Content-Type-Options "nosniff"
		Referrer-Policy "no-referrer"
		Permissions-Policy "camera=(), microphone=(), geolocation=()"
		Strict-Transport-Security "max-age=31536000"
		Cross-Origin-Opener-Policy "same-origin"
		-Server
	}

	log {
		output file /var/log/caddy/remote.roalt.dev.log {
			roll_size 10MiB
			roll_keep 3
			roll_keep_for 72h
		}
	}
}
```

- `log_skip` (Caddy 2.9+; `skip_log` before) keeps relay connections out of the access log.
- `header_up X-Real-IP {remote_host}` overwrites any client-sent value. The relay ignores every other forwarding header.
- The CSP allows only same-origin scripts and styles, and only the production relay socket. Do not widen `connect-src`. The hosted dashboard ignores `?relay=`, and QR links never carry a relay; custom relays are for local dev builds only.
- `roalt.dev` keeps serving the marketing site and must no longer serve the dashboard or proxy `/ws/remote`.
