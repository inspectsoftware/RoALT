use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use roalt_remote_relay::{BIN_VERSION, MANUAL_MISS_MAX, MANUAL_MISS_SHARE, MAX_SUBSCRIPTIONS, RateLimit, RelayConfig, RelayState, pc_id_for_secret_hash, router, sha256_hex};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message as TMsg;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const WAIT: Duration = Duration::from_secs(5);

async fn spawn_with(config: RelayConfig) -> (SocketAddr, RelayState) {
    let state = RelayState::with_config(config);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    (addr, state)
}

fn roomy() -> RelayConfig {
    let unlimited = RateLimit { bytes_per_sec: f64::MAX, burst_bytes: f64::MAX, msgs_per_sec: 1e9, burst_msgs: 1e9 };
    RelayConfig {
        max_connections_per_ip: 4096,
        max_connections_per_v6_56: 4096,
        max_connections_per_site: 4096,
        max_connections_per_v6_32: 4096,
        connect_rate: unlimited,
        ..RelayConfig::default()
    }
}

async fn spawn() -> (SocketAddr, RelayState) {
    spawn_with(roomy()).await
}

fn url(addr: SocketAddr) -> String {
    format!("ws://{addr}/ws/remote")
}

async fn connect(addr: SocketAddr) -> Ws {
    tokio_tungstenite::connect_async(url(addr)).await.unwrap().0
}

async fn send(ws: &mut Ws, value: Value) {
    ws.send(TMsg::Text(value.to_string().into())).await.unwrap();
}

async fn next_json(ws: &mut Ws) -> Value {
    loop {
        let msg = tokio::time::timeout(WAIT, ws.next()).await.expect("timed out waiting for json").unwrap().unwrap();
        match msg {
            TMsg::Text(t) => return serde_json::from_str(&t).unwrap(),
            TMsg::Ping(_) | TMsg::Pong(_) => continue,
            other => panic!("expected text json, got {other:?}"),
        }
    }
}

async fn expect_closed(ws: &mut Ws) {
    loop {
        match tokio::time::timeout(WAIT, ws.next()).await.expect("socket stayed open") {
            None | Some(Err(_)) | Some(Ok(TMsg::Close(_))) => return,
            Some(Ok(TMsg::Ping(_) | TMsg::Pong(_))) => continue,
            Some(Ok(other)) => panic!("expected close, got {other:?}"),
        }
    }
}

async fn assert_alive(ws: &mut Ws) {
    send(ws, json!({ "t": "ping" })).await;
    assert_eq!(next_json(ws).await["t"], "pong");
}

async fn pc_hello(addr: SocketAddr, secret: &str, room_tokens: &[&str]) -> (Ws, Uuid) {
    let mut ws = connect(addr).await;
    let secret_hash = sha256_hex(secret);
    let pc_id = pc_id_for_secret_hash(&secret_hash);
    let tokens: Vec<String> = room_tokens.iter().map(|t| sha256_hex(t)).collect();
    send(
        &mut ws,
        json!({ "t": "pc_hello", "pcId": pc_id, "pcSecretHash": secret_hash, "allowedTokens": tokens, "proto": 2, "app": "1.3.0" }),
    )
    .await;
    assert_eq!(next_json(&mut ws).await["t"], "pc_ready");
    (ws, pc_id)
}

fn sub_item(pc_id: Uuid, device_id: Uuid, token: &str) -> Value {
    json!({ "pcId": pc_id, "deviceId": device_id, "roomToken": token })
}

async fn subscribe(ws: &mut Ws, items: Vec<Value>) {
    send(ws, json!({ "t": "subscribe", "pcs": items, "proto": 2 })).await;
}

async fn attached_viewer(addr: SocketAddr, pc: &mut Ws, pc_id: Uuid, token: &str) -> (Ws, Uuid) {
    let mut viewer = connect(addr).await;
    subscribe(&mut viewer, vec![sub_item(pc_id, Uuid::new_v4(), token)]).await;
    assert_eq!(next_json(&mut viewer).await["t"], "pc_online");
    let joined = next_json(pc).await;
    assert_eq!(joined["t"], "viewer_joined");
    (viewer, Uuid::parse_str(joined["viewerId"].as_str().unwrap()).unwrap())
}

fn frame(target: Uuid, payload_len: usize) -> TMsg {
    let mut f = vec![BIN_VERSION];
    f.extend_from_slice(target.as_bytes());
    f.resize(17 + payload_len, 0xAB);
    TMsg::Binary(f.into())
}

#[tokio::test]
async fn pairing_and_authenticated_binary_routing() {
    let (addr, _state) = spawn().await;
    let (mut pc, pc_id) = pc_hello(addr, "pc-secret", &[]).await;
    let mut viewer = connect(addr).await;
    let device_id = Uuid::new_v4();
    let room_token = "tok-123456";

    send(&mut pc, json!({ "t": "pair_open", "pairId": "00000000000a1b2c", "manualCode": "ABCD-1234" })).await;
    assert_eq!(next_json(&mut pc).await["t"], "pair_opened");

    send(&mut viewer, json!({ "t": "pair_join", "pairId": "abcd1234", "data": "cipher-join", "proto": 2 })).await;
    let joined = next_json(&mut pc).await;
    assert_eq!(joined["t"], "pair_joined");
    assert_eq!(joined["data"], "cipher-join");
    let viewer_id = joined["viewerId"].as_str().unwrap().to_owned();

    send(&mut pc, json!({ "t": "pair_msg", "pairId": "00000000000a1b2c", "viewerId": viewer_id, "data": "cipher-ack" })).await;
    let ack = next_json(&mut viewer).await;
    assert_eq!(ack["t"], "pair_msg");
    assert_eq!(ack["data"], "cipher-ack");

    send(&mut pc, json!({ "t": "pc_tokens", "allowedTokens": [sha256_hex(room_token)] })).await;
    assert_eq!(next_json(&mut pc).await["t"], "pc_tokens_ok");

    subscribe(&mut viewer, vec![sub_item(pc_id, device_id, "wrong")]).await;
    assert_eq!(next_json(&mut viewer).await["t"], "pc_offline");

    subscribe(&mut viewer, vec![sub_item(pc_id, device_id, room_token)]).await;
    assert_eq!(next_json(&mut viewer).await["t"], "pc_online");
    let vj = next_json(&mut pc).await;
    assert_eq!(vj["t"], "viewer_joined");
    let v_uuid = Uuid::parse_str(vj["viewerId"].as_str().unwrap()).unwrap();

    let mut f = vec![BIN_VERSION];
    f.extend_from_slice(pc_id.as_bytes());
    f.extend_from_slice(b"hello-e2ee");
    viewer.send(TMsg::Binary(f.into())).await.unwrap();
    let TMsg::Binary(b) = pc.next().await.unwrap().unwrap() else { panic!("expected binary") };
    assert_eq!(b[0], BIN_VERSION);
    assert_eq!(&b[1..17], v_uuid.as_bytes());
    assert_eq!(&b[17..], b"hello-e2ee");

    send(&mut pc, json!({ "t": "pc_tokens", "allowedTokens": [] })).await;
    assert_eq!(next_json(&mut viewer).await["t"], "pc_offline");
    let mut replies = [next_json(&mut pc).await["t"].clone(), next_json(&mut pc).await["t"].clone()];
    replies.sort_by_key(|t| t.to_string());
    assert_eq!(replies, [json!("pc_tokens_ok"), json!("viewer_left")]);
}

#[test]
fn pc_id_rule_vector() {
    assert_eq!(pc_id_for_secret_hash(&sha256_hex("pc-secret")).to_string(), "d5d6cc8b-7d41-ca38-749e-6786be4d1849");
}

#[tokio::test]
async fn hijacking_online_pc_is_rejected() {
    let (addr, _state) = spawn().await;
    let (_pc1, pc_id) = pc_hello(addr, "good", &[]).await;
    let mut pc2 = connect(addr).await;
    send(&mut pc2, json!({ "t": "pc_hello", "pcId": pc_id, "pcSecretHash": sha256_hex("evil"), "proto": 2 })).await;
    assert_eq!(next_json(&mut pc2).await["code"], "forbidden");
}

#[tokio::test]
async fn pair_open_cannot_take_another_pcs_room_or_code() {
    let (addr, _state) = spawn().await;
    let (mut victim, _) = pc_hello(addr, "victim", &[]).await;
    let (mut attacker, _) = pc_hello(addr, "attacker", &[]).await;

    send(&mut victim, json!({ "t": "pair_open", "pairId": "00000000000000f1", "manualCode": "ABCD-2345" })).await;
    assert_eq!(next_json(&mut victim).await["t"], "pair_opened");

    send(&mut attacker, json!({ "t": "pair_open", "pairId": "00000000000000f1" })).await;
    let taken = next_json(&mut attacker).await;
    assert_eq!(taken, json!({ "t": "err", "code": "pair_taken", "pairId": "00000000000000f1" }));

    send(&mut attacker, json!({ "t": "pair_open", "pairId": "00000000000000a1", "manualCode": "abcd2345" })).await;
    let taken = next_json(&mut attacker).await;
    assert_eq!(taken, json!({ "t": "err", "code": "pair_taken", "pairId": "00000000000000a1" }));

    let mut phone = connect(addr).await;
    send(&mut phone, json!({ "t": "pair_join", "pairId": "00000000000000a1", "data": "x", "proto": 2 })).await;
    assert_eq!(next_json(&mut phone).await["code"], "not_found");

    let mut phone = connect(addr).await;
    send(&mut phone, json!({ "t": "pair_join", "pairId": "ABCD-2345", "data": "join", "proto": 2 })).await;
    let joined = next_json(&mut victim).await;
    assert_eq!(joined["t"], "pair_joined");
    assert_eq!(joined["pairId"], "00000000000000f1");

    let mut second = connect(addr).await;
    send(&mut second, json!({ "t": "pair_join", "pairId": "ABCD-2345", "data": "late", "proto": 2 })).await;
    assert_eq!(next_json(&mut second).await, json!({ "t": "pair_err", "code": "busy" }));
    let contended = next_json(&mut victim).await;
    assert_eq!(contended["t"], "pair_joined");
    assert_eq!(contended["data"], "late");
    assert_ne!(contended["viewerId"], joined["viewerId"]);
    send(&mut victim, json!({ "t": "pair_msg", "pairId": "00000000000000f1", "viewerId": contended["viewerId"], "data": "leak" })).await;
    assert_alive(&mut victim).await;
    assert_alive(&mut second).await;

    send(&mut attacker, json!({ "t": "pair_open", "pairId": "00000000000000a2", "manualCode": "WXYZ-6789" })).await;
    assert_eq!(next_json(&mut attacker).await["t"], "pair_opened");

    send(&mut victim, json!({ "t": "pair_close", "pairId": "00000000000000f1" })).await;
    assert_alive(&mut victim).await;
    send(&mut attacker, json!({ "t": "pair_open", "pairId": "00000000000000a3", "manualCode": "ABCD-2345" })).await;
    assert_eq!(next_json(&mut attacker).await["t"], "pair_opened");

    let mut phone = connect(addr).await;
    send(&mut phone, json!({ "t": "pair_join", "pairId": "WXYZ6789", "data": "x", "proto": 2 })).await;
    assert_eq!(next_json(&mut phone).await["code"], "not_found");
}

#[tokio::test]
async fn stalled_viewer_cannot_grow_relay_memory() {
    let unlimited = RateLimit { bytes_per_sec: 1e12, burst_bytes: 1e12, msgs_per_sec: 1e9, burst_msgs: 1e9 };
    let config = RelayConfig {
        write_timeout: Duration::from_secs(60),
        queue_age_limit: Duration::from_secs(60),
        pc_rate: unlimited,
        ip_rate: unlimited,
        ..RelayConfig::default()
    };
    let budget = config.socket_queue_budget;
    let (addr, state) = spawn_with(config).await;
    let (mut pc, pc_id) = pc_hello(addr, "pc", &["t", "u", "v", "w"]).await;
    let mut stalled = Vec::new();
    for token in ["t", "u", "v", "w"] {
        let (viewer, viewer_id) = attached_viewer(addr, &mut pc, pc_id, token).await;
        stalled.push((viewer, viewer_id));
    }

    let mut peak = 0;
    for _ in 0..60 {
        for (_, viewer_id) in &stalled {
            pc.send(frame(*viewer_id, 200 * 1024)).await.unwrap();
        }
        peak = peak.max(state.queued_bytes());
    }
    assert_alive(&mut pc).await;
    peak = peak.max(state.queued_bytes());

    let per_viewer_cap = budget + 256 * 1024;
    assert!(peak <= stalled.len() * per_viewer_cap, "queued {peak} bytes for {} stalled viewers", stalled.len());
    assert!(peak > 0);
}

#[tokio::test]
async fn silent_sockets_hit_registration_and_idle_timeouts() {
    let config = RelayConfig {
        registration_timeout: Duration::from_millis(300),
        idle_timeout: Duration::from_millis(600),
        ping_interval: Duration::from_secs(60),
        ..RelayConfig::default()
    };
    let (addr, state) = spawn_with(config).await;

    let mut unregistered = connect(addr).await;
    let (mut idle_pc, _) = pc_hello(addr, "idle", &[]).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    expect_closed(&mut unregistered).await;
    expect_closed(&mut idle_pc).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(state.connections(), 0);
    assert_eq!(state.stats().0, 0);
}

#[tokio::test]
async fn relay_sends_ws_pings() {
    let config = RelayConfig { ping_interval: Duration::from_millis(100), ..RelayConfig::default() };
    let (addr, _state) = spawn_with(config).await;
    let (mut pc, _) = pc_hello(addr, "pinged", &[]).await;
    let msg = tokio::time::timeout(WAIT, pc.next()).await.unwrap().unwrap().unwrap();
    assert!(matches!(msg, TMsg::Ping(_)), "got {msg:?}");
}

#[tokio::test]
async fn old_protocol_gets_upgrade_required() {
    let (addr, _state) = spawn().await;

    let mut pc = connect(addr).await;
    let secret_hash = sha256_hex("old");
    let pc_id = pc_id_for_secret_hash(&secret_hash);
    send(&mut pc, json!({ "t": "pc_hello", "pcId": pc_id, "pcSecretHash": secret_hash, "allowedTokens": [] })).await;
    assert_eq!(next_json(&mut pc).await, json!({ "t": "err", "code": "upgrade_required" }));
    expect_closed(&mut pc).await;

    let mut viewer = connect(addr).await;
    send(&mut viewer, json!({ "t": "subscribe", "pcs": [], "proto": 1 })).await;
    assert_eq!(next_json(&mut viewer).await["code"], "upgrade_required");
    expect_closed(&mut viewer).await;

    let mut joiner = connect(addr).await;
    send(&mut joiner, json!({ "t": "pair_join", "pairId": "ABCD2345", "data": "x" })).await;
    assert_eq!(next_json(&mut joiner).await["code"], "upgrade_required");
    expect_closed(&mut joiner).await;
}

#[tokio::test]
async fn subscriptions_over_cap_get_too_many_and_bad_items_are_skipped() {
    let (addr, _state) = spawn().await;
    let (_pc, pc_id) = pc_hello(addr, "real", &["tok"]).await;
    let mut viewer = connect(addr).await;

    let mut items = vec![
        json!({ "pcId": "not-a-uuid", "deviceId": Uuid::new_v4(), "roomToken": "x" }),
        json!({ "pcId": Uuid::new_v4(), "deviceId": Uuid::new_v4(), "roomToken": 5 }),
        sub_item(pc_id, Uuid::new_v4(), "tok"),
    ];
    let offline: Vec<Uuid> = (0..MAX_SUBSCRIPTIONS).map(|_| Uuid::new_v4()).collect();
    items.extend(offline.iter().map(|id| sub_item(*id, Uuid::new_v4(), "x")));
    subscribe(&mut viewer, items).await;

    let mut online = 0;
    let mut offline_count = 0;
    let mut too_many = Vec::new();
    for _ in 0..=MAX_SUBSCRIPTIONS {
        let msg = next_json(&mut viewer).await;
        match msg["t"].as_str().unwrap() {
            "pc_online" => online += 1,
            "pc_offline" => offline_count += 1,
            "pc_err" => {
                assert_eq!(msg["code"], "too_many");
                too_many.push(msg["pcId"].as_str().unwrap().to_owned());
            }
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!(online, 1);
    assert_eq!(offline_count, MAX_SUBSCRIPTIONS - 1);
    assert_eq!(too_many, vec![offline.last().unwrap().to_string()]);
}

#[tokio::test]
async fn browser_origins_are_checked() {
    let (addr, _state) = spawn().await;
    let with_origin = |origin: &'static str| {
        let mut req = url(addr).into_client_request().unwrap();
        req.headers_mut().insert("Origin", origin.parse().unwrap());
        req
    };

    let refused = tokio_tungstenite::connect_async(with_origin("https://evil.example")).await;
    match refused {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => assert_eq!(resp.status(), 403),
        other => panic!("expected 403, got {:?}", other.map(|_| ())),
    }
    let refused = tokio_tungstenite::connect_async(with_origin("https://roalt.dev")).await;
    assert!(refused.is_err());

    for origin in ["https://remote.roalt.dev", "http://localhost:5175", "http://127.0.0.1:5175"] {
        let (mut ws, _) = tokio_tungstenite::connect_async(with_origin(origin)).await.unwrap();
        assert_alive(&mut ws).await;
    }
    let mut native = connect(addr).await;
    assert_alive(&mut native).await;
}

#[tokio::test]
async fn connections_are_capped_per_ip() {
    let config = RelayConfig { max_connections_per_ip: 2, ..RelayConfig::default() };
    let (addr, state) = spawn_with(config).await;
    let _a = connect(addr).await;
    let _b = connect(addr).await;
    match tokio_tungstenite::connect_async(url(addr)).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => assert_eq!(resp.status(), 429),
        other => panic!("expected 429, got {:?}", other.map(|_| ())),
    }
    assert_eq!(state.connections(), 2);
}

#[tokio::test]
async fn connections_are_capped_globally() {
    let config = RelayConfig { max_connections: 1, ..RelayConfig::default() };
    let (addr, _state) = spawn_with(config).await;
    let _a = connect(addr).await;
    match tokio_tungstenite::connect_async(url(addr)).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => assert_eq!(resp.status(), 503),
        other => panic!("expected 503, got {:?}", other.map(|_| ())),
    }
}

#[tokio::test]
async fn oversized_and_bad_frames_get_errors_without_closing() {
    let (addr, _state) = spawn().await;
    let (mut pc, pc_id) = pc_hello(addr, "frames", &["t"]).await;
    let (mut viewer, viewer_id) = attached_viewer(addr, &mut pc, pc_id, "t").await;

    viewer.send(frame(pc_id, 70 * 1024)).await.unwrap();
    assert_eq!(next_json(&mut viewer).await["code"], "frame_too_large");
    assert_alive(&mut viewer).await;

    pc.send(frame(viewer_id, 256 * 1024)).await.unwrap();
    assert_eq!(next_json(&mut pc).await["code"], "frame_too_large");
    assert_alive(&mut pc).await;

    send(&mut viewer, json!({ "t": "ping", "pad": "x".repeat(40 * 1024) })).await;
    assert_eq!(next_json(&mut viewer).await["code"], "frame_too_large");
    assert_alive(&mut viewer).await;

    let mut bad = vec![0x02];
    bad.extend_from_slice(pc_id.as_bytes());
    bad.extend_from_slice(b"x");
    viewer.send(TMsg::Binary(bad.into())).await.unwrap();
    assert_eq!(next_json(&mut viewer).await["code"], "bad_version");
    assert_alive(&mut viewer).await;

    pc.send(frame(viewer_id, 1024)).await.unwrap();
    let TMsg::Binary(b) = viewer.next().await.unwrap().unwrap() else { panic!("expected binary") };
    assert_eq!(&b[1..17], pc_id.as_bytes());
}

#[tokio::test]
async fn binary_before_registration_closes_the_socket() {
    let (addr, _state) = spawn().await;
    let mut ws = connect(addr).await;
    ws.send(frame(Uuid::new_v4(), 8)).await.unwrap();
    expect_closed(&mut ws).await;
}

#[tokio::test]
async fn revoked_device_cannot_tell_online_from_offline() {
    let (addr, _state) = spawn().await;
    let (pc, pc_id) = pc_hello(addr, "watched", &["good"]).await;
    let mut revoked = connect(addr).await;
    subscribe(&mut revoked, vec![sub_item(pc_id, Uuid::new_v4(), "old-token")]).await;
    assert_eq!(next_json(&mut revoked).await, json!({ "t": "pc_offline", "pcId": pc_id }));

    drop(pc);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (_pc_again, _) = pc_hello(addr, "watched", &["good"]).await;
    assert_alive(&mut revoked).await;
}

#[tokio::test]
async fn viewers_per_pc_are_capped_and_a_freed_slot_goes_to_a_waiting_viewer() {
    let (addr, _state) = spawn().await;
    let tokens: Vec<String> = (0..17).map(|i| format!("t{i}")).collect();
    let token_refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
    let (mut pc, pc_id) = pc_hello(addr, "popular", &token_refs).await;
    let mut viewers = Vec::new();
    for token in &token_refs[..16] {
        viewers.push(attached_viewer(addr, &mut pc, pc_id, token).await);
    }
    let mut extra = connect(addr).await;
    subscribe(&mut extra, vec![sub_item(pc_id, Uuid::new_v4(), token_refs[16])]).await;
    assert_eq!(next_json(&mut extra).await, json!({ "t": "pc_err", "pcId": pc_id, "code": "full" }));

    let (leaving, leaving_id) = viewers.remove(0);
    drop(leaving);
    assert_eq!(next_json(&mut pc).await, json!({ "t": "viewer_left", "viewerId": leaving_id }));
    assert_eq!(next_json(&mut pc).await["t"], "viewer_joined");
    assert_eq!(next_json(&mut extra).await["t"], "pc_online");
}

#[tokio::test]
async fn one_device_token_holds_at_most_three_sockets() {
    let (addr, _state) = spawn().await;
    let (mut pc, pc_id) = pc_hello(addr, "busy-device", &["tok"]).await;
    let (mut oldest, oldest_id) = attached_viewer(addr, &mut pc, pc_id, "tok").await;
    let _second = attached_viewer(addr, &mut pc, pc_id, "tok").await;
    let _third = attached_viewer(addr, &mut pc, pc_id, "tok").await;

    let mut fourth = connect(addr).await;
    subscribe(&mut fourth, vec![sub_item(pc_id, Uuid::new_v4(), "tok")]).await;
    assert_eq!(next_json(&mut fourth).await["t"], "pc_online");
    assert_eq!(next_json(&mut oldest).await, json!({ "t": "pc_err", "pcId": pc_id, "code": "replaced" }));
    assert_eq!(next_json(&mut pc).await, json!({ "t": "viewer_left", "viewerId": oldest_id }));
    let joined = next_json(&mut pc).await;
    assert_eq!(joined["t"], "viewer_joined");
    assert_eq!(joined["tokenHash"], sha256_hex("tok"));

    send(&mut pc, json!({ "t": "pc_tokens", "allowedTokens": [sha256_hex("tok")] })).await;
    assert_eq!(next_json(&mut pc).await["t"], "pc_tokens_ok");
    assert_alive(&mut oldest).await;
}

#[tokio::test]
async fn kicked_viewers_stay_out_until_they_ask_for_a_restart() {
    let (addr, _state) = spawn_with(roomy()).await;
    let (mut pc, pc_id) = pc_hello(addr, "kicker", &["k"]).await;
    let mut viewer = connect(addr).await;
    let item = sub_item(pc_id, Uuid::new_v4(), "k");
    subscribe(&mut viewer, vec![item.clone()]).await;
    assert_eq!(next_json(&mut viewer).await["t"], "pc_online");
    let viewer_id = next_json(&mut pc).await["viewerId"].clone();

    send(&mut pc, json!({ "t": "kick_viewer", "viewerId": viewer_id })).await;
    assert_eq!(next_json(&mut viewer).await["t"], "pc_offline");
    assert_eq!(next_json(&mut pc).await["t"], "viewer_left");
    send(&mut pc, json!({ "t": "pc_tokens", "allowedTokens": [sha256_hex("k")] })).await;
    assert_eq!(next_json(&mut pc).await["t"], "pc_tokens_ok");

    subscribe(&mut viewer, vec![item.clone()]).await;
    assert_eq!(next_json(&mut viewer).await, json!({ "t": "pc_offline", "pcId": pc_id }), "a subscribe for other PCs leaves the kick alone");
    assert_alive(&mut pc).await;

    let mut restart = item.clone();
    restart["restart"] = json!(true);
    subscribe(&mut viewer, vec![restart]).await;
    assert_eq!(next_json(&mut viewer).await["t"], "pc_online", "the dashboard's own retry after a quiet drop gets back in at once");
    assert_eq!(next_json(&mut pc).await["t"], "viewer_joined");
}

#[tokio::test]
async fn a_kick_with_a_reason_reaches_the_viewer_as_that_reason() {
    let (addr, _state) = spawn_with(roomy()).await;
    let (mut pc, pc_id) = pc_hello(addr, "busy", &["k"]).await;
    let (mut viewer, viewer_id) = attached_viewer(addr, &mut pc, pc_id, "k").await;
    send(&mut pc, json!({ "t": "kick_viewer", "viewerId": viewer_id, "code": "device_sessions" })).await;
    assert_eq!(next_json(&mut viewer).await, json!({ "t": "pc_err", "pcId": pc_id, "code": "device_sessions" }));
    assert_eq!(next_json(&mut pc).await["t"], "viewer_left");

    let (mut other, other_id) = attached_viewer(addr, &mut pc, pc_id, "k").await;
    send(&mut pc, json!({ "t": "kick_viewer", "viewerId": other_id, "code": "<b>made up</b>" })).await;
    assert_eq!(next_json(&mut other).await, json!({ "t": "pc_offline", "pcId": pc_id }), "only known reasons are passed on");
}

#[tokio::test]
async fn a_replaced_socket_retrying_does_not_replace_the_next_one() {
    let (addr, _state) = spawn().await;
    let (mut pc, pc_id) = pc_hello(addr, "four-tabs", &["tab"]).await;
    let mut tabs = Vec::new();
    for _ in 0..3 {
        tabs.push(attached_viewer(addr, &mut pc, pc_id, "tab").await.0);
    }
    let mut fourth = connect(addr).await;
    subscribe(&mut fourth, vec![sub_item(pc_id, Uuid::new_v4(), "tab")]).await;
    assert_eq!(next_json(&mut fourth).await["t"], "pc_online");
    assert_eq!(next_json(&mut tabs[0]).await, json!({ "t": "pc_err", "pcId": pc_id, "code": "replaced" }), "the dashboard stops retrying on this");
    assert_eq!(next_json(&mut pc).await["t"], "viewer_left");
    assert_eq!(next_json(&mut pc).await["t"], "viewer_joined");

    subscribe(&mut tabs[0], vec![sub_item(pc_id, Uuid::new_v4(), "tab")]).await;
    assert_eq!(next_json(&mut tabs[0]).await["t"], "pc_offline");
    assert_alive(&mut pc).await;
    assert_alive(&mut tabs[1]).await;
}

#[tokio::test]
async fn a_pair_id_shaped_like_a_manual_code_cannot_capture_its_joins() {
    let (addr, _state) = spawn().await;
    let (mut victim, _) = pc_hello(addr, "code-victim", &[]).await;
    let (mut attacker, _) = pc_hello(addr, "code-thief", &[]).await;
    for alias in ["abcd-2345", "abcd2345", "ABCD-2345"] {
        send(&mut attacker, json!({ "t": "pair_open", "pairId": alias })).await;
        assert_eq!(next_json(&mut attacker).await, json!({ "t": "pair_err", "code": "bad_pair_id" }));
    }
    send(&mut victim, json!({ "t": "pair_open", "pairId": "00000000000000f2", "manualCode": "ABCD-2345" })).await;
    assert_eq!(next_json(&mut victim).await["t"], "pair_opened");

    let mut phones = Vec::new();
    for typed in ["ABCD-2345", "abcd2345"] {
        let mut phone = connect(addr).await;
        send(&mut phone, json!({ "t": "pair_join", "pairId": typed, "data": typed, "proto": 2 })).await;
        phones.push(phone);
        let joined = next_json(&mut victim).await;
        assert_eq!(joined["pairId"], "00000000000000f2");
        assert_eq!(joined["data"], typed);
    }
    assert_alive(&mut attacker).await;
}

#[tokio::test]
async fn the_pc_hears_when_its_joiner_leaves() {
    let (addr, _state) = spawn().await;
    let (mut pc, _) = pc_hello(addr, "lonely", &[]).await;
    send(&mut pc, json!({ "t": "pair_open", "pairId": "00000000000000f3", "manualCode": "WXYZ-2345" })).await;
    assert_eq!(next_json(&mut pc).await["t"], "pair_opened");
    let mut phone = connect(addr).await;
    send(&mut phone, json!({ "t": "pair_join", "pairId": "WXYZ2345", "data": "x", "proto": 2 })).await;
    let viewer_id = next_json(&mut pc).await["viewerId"].clone();
    drop(phone);
    assert_eq!(next_json(&mut pc).await, json!({ "t": "pair_left", "pairId": "00000000000000f3", "viewerId": viewer_id }));
}

#[tokio::test]
async fn viewers_attached_to_no_pc_are_closed() {
    let config = RelayConfig { purposeless_timeout: Duration::from_millis(300), ..RelayConfig::default() };
    let (addr, _state) = spawn_with(config).await;
    let (mut pc, pc_id) = pc_hello(addr, "purpose", &["yes"]).await;
    let mut empty = connect(addr).await;
    subscribe(&mut empty, vec![]).await;
    let mut refused = connect(addr).await;
    subscribe(&mut refused, vec![sub_item(pc_id, Uuid::new_v4(), "nope")]).await;
    assert_eq!(next_json(&mut refused).await["t"], "pc_offline");
    let (mut attached, _) = attached_viewer(addr, &mut pc, pc_id, "yes").await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    for ws in [&mut empty, &mut refused] {
        assert_eq!(next_json(ws).await, json!({ "t": "err", "code": "no_purpose" }));
        expect_closed(ws).await;
    }
    assert_alive(&mut attached).await;
}

#[tokio::test]
async fn a_device_waiting_for_a_slot_on_a_full_pc_is_kept() {
    let config = RelayConfig { purposeless_timeout: Duration::from_millis(300), ..roomy() };
    let (addr, _state) = spawn_with(config).await;
    let tokens: Vec<String> = (0..17).map(|i| format!("w{i}")).collect();
    let token_refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
    let (mut pc, pc_id) = pc_hello(addr, "crowded", &token_refs).await;
    let mut viewers = Vec::new();
    for token in &token_refs[..16] {
        viewers.push(attached_viewer(addr, &mut pc, pc_id, token).await);
    }
    let mut waiting = connect(addr).await;
    subscribe(&mut waiting, vec![sub_item(pc_id, Uuid::new_v4(), token_refs[16])]).await;
    assert_eq!(next_json(&mut waiting).await["code"], "full");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_alive(&mut waiting).await;
}

#[tokio::test]
async fn a_viewer_attached_briefly_gets_the_full_timeout_after_it_loses_its_pc() {
    let timeout = Duration::from_millis(600);
    let (addr, _state) = spawn_with(RelayConfig { purposeless_timeout: timeout, ..roomy() }).await;
    let secret = "brief";
    let pc_id = pc_id_for_secret_hash(&sha256_hex(secret));
    let mut viewer = connect(addr).await;
    subscribe(&mut viewer, vec![sub_item(pc_id, Uuid::new_v4(), "b")]).await;
    assert_eq!(next_json(&mut viewer).await["t"], "pc_offline");
    tokio::time::sleep(timeout / 2).await;
    let (pc, _) = pc_hello(addr, secret, &["b"]).await;
    assert_eq!(next_json(&mut viewer).await["t"], "pc_online");
    tokio::time::sleep(timeout / 2).await;
    drop(pc);
    assert_eq!(next_json(&mut viewer).await["t"], "pc_offline");
    tokio::time::sleep(timeout / 2).await;
    assert_alive(&mut viewer).await;
}

#[tokio::test]
async fn a_joiner_hears_when_the_pc_closes_its_room() {
    let (addr, _state) = spawn().await;
    let (mut pc, _) = pc_hello(addr, "closer", &[]).await;
    send(&mut pc, json!({ "t": "pair_open", "pairId": "00000000000000c1" })).await;
    assert_eq!(next_json(&mut pc).await["t"], "pair_opened");
    let mut phone = connect(addr).await;
    send(&mut phone, json!({ "t": "pair_join", "pairId": "00000000000000c1", "data": "x", "proto": 2 })).await;
    assert_eq!(next_json(&mut pc).await["t"], "pair_joined");
    send(&mut pc, json!({ "t": "pair_close", "pairId": "00000000000000c1" })).await;
    assert_eq!(next_json(&mut phone).await, json!({ "t": "pair_err", "code": "closed" }));
}

#[tokio::test]
async fn a_room_holder_cannot_flood_its_pc_with_rejoins() {
    let (addr, _state) = spawn().await;
    let (mut pc, _) = pc_hello(addr, "rejoined", &[]).await;
    send(&mut pc, json!({ "t": "pair_open", "pairId": "00000000000000d1" })).await;
    assert_eq!(next_json(&mut pc).await["t"], "pair_opened");
    let mut phone = connect(addr).await;
    for _ in 0..6 {
        send(&mut phone, json!({ "t": "pair_join", "pairId": "00000000000000d1", "data": "x", "proto": 2 })).await;
    }
    assert_eq!(next_json(&mut phone).await, json!({ "t": "pair_err", "code": "busy" }));
    for _ in 0..4 {
        assert_eq!(next_json(&mut pc).await["t"], "pair_joined");
    }
    assert_alive(&mut pc).await;
}

#[tokio::test]
async fn subscribing_to_made_up_pc_ids_does_not_keep_a_busy_socket_open() {
    let config = RelayConfig { purposeless_timeout: Duration::from_millis(300), ..RelayConfig::default() };
    let (addr, _state) = spawn_with(config).await;
    let mut squatter = connect(addr).await;
    subscribe(&mut squatter, (0..8).map(|_| sub_item(Uuid::new_v4(), Uuid::new_v4(), "x")).collect()).await;
    closes_while_pinging(&mut squatter).await;
}

async fn closes_while_pinging(ws: &mut Ws) {
    let started = std::time::Instant::now();
    loop {
        assert!(started.elapsed() < Duration::from_secs(3), "a socket with nothing to do stayed open");
        if ws.send(TMsg::Text(json!({ "t": "ping" }).to_string().into())).await.is_err() {
            return;
        }
        if let Ok(None | Some(Err(_)) | Some(Ok(TMsg::Close(_)))) = tokio::time::timeout(Duration::from_millis(100), ws.next()).await {
            return;
        }
    }
}

#[tokio::test]
async fn a_stream_of_frames_to_unattached_targets_closes_the_socket() {
    let (addr, _state) = spawn().await;
    let (mut pc, pc_id) = pc_hello(addr, "probed", &["p"]).await;
    let (mut viewer, _) = attached_viewer(addr, &mut pc, pc_id, "p").await;
    for _ in 0..4 {
        viewer.send(frame(Uuid::new_v4(), 8)).await.unwrap();
    }
    assert_alive(&mut viewer).await;
    for _ in 0..70 {
        viewer.send(frame(Uuid::new_v4(), 8)).await.unwrap();
    }
    expect_closed(&mut viewer).await;
}

#[tokio::test]
async fn too_old_apps_are_refused_when_a_minimum_is_set() {
    let (addr, _state) = spawn_with(RelayConfig { min_app_version: Some((1, 4, 0)), ..RelayConfig::default() }).await;
    let mut old = connect(addr).await;
    let secret_hash = sha256_hex("old-app");
    let pc_id = pc_id_for_secret_hash(&secret_hash);
    send(&mut old, json!({ "t": "pc_hello", "pcId": pc_id, "pcSecretHash": secret_hash, "proto": 2, "app": "1.3.11" })).await;
    assert_eq!(next_json(&mut old).await, json!({ "t": "err", "code": "upgrade_required", "min": "1.4.0" }));
    expect_closed(&mut old).await;

    let mut new = connect(addr).await;
    send(&mut new, json!({ "t": "pc_hello", "pcId": pc_id, "pcSecretHash": secret_hash, "proto": 2, "app": "1.4.0" })).await;
    assert_eq!(next_json(&mut new).await["t"], "pc_ready");
}

#[tokio::test]
async fn a_flooding_socket_is_slowed_down_not_served_at_line_rate() {
    let tight = RateLimit { bytes_per_sec: 1e6, burst_bytes: 1e6, msgs_per_sec: 20.0, burst_msgs: 5.0 };
    let (addr, _state) = spawn_with(RelayConfig { viewer_rate: tight, ..RelayConfig::default() }).await;
    let mut ws = connect(addr).await;
    let started = std::time::Instant::now();
    for _ in 0..25 {
        send(&mut ws, json!({ "t": "ping" })).await;
    }
    let mut pongs = 0;
    while pongs < 25 {
        match next_json(&mut ws).await["t"].as_str() {
            Some("pong") => pongs += 1,
            Some("err") => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(started.elapsed() >= Duration::from_millis(900), "25 pings at 20/s after a burst of 5 took {:?}", started.elapsed());
}

#[tokio::test]
async fn token_bucket_sends_rate_limited() {
    let tight = RateLimit { bytes_per_sec: 1e6, burst_bytes: 1e6, msgs_per_sec: 0.1, burst_msgs: 2.0 };
    let (addr, _state) = spawn_with(RelayConfig { viewer_rate: tight, ..RelayConfig::default() }).await;
    let mut ws = connect(addr).await;
    for _ in 0..4 {
        send(&mut ws, json!({ "t": "ping" })).await;
    }
    assert_eq!(next_json(&mut ws).await["t"], "pong");
    assert_eq!(next_json(&mut ws).await["t"], "pong");
    assert_eq!(next_json(&mut ws).await, json!({ "t": "err", "code": "rate_limited" }));
}

#[tokio::test]
async fn pc_tokens_attach_waiting_subscribers() {
    let (addr, _state) = spawn().await;
    let (mut pc, pc_id) = pc_hello(addr, "late-token", &[]).await;
    let mut viewer = connect(addr).await;
    subscribe(&mut viewer, vec![sub_item(pc_id, Uuid::new_v4(), "fresh")]).await;
    assert_eq!(next_json(&mut viewer).await["t"], "pc_offline");

    send(&mut pc, json!({ "t": "pc_tokens", "allowedTokens": [sha256_hex("fresh")] })).await;
    let mut replies = [next_json(&mut pc).await["t"].clone(), next_json(&mut pc).await["t"].clone()];
    replies.sort_by_key(|t| t.to_string());
    assert_eq!(replies, [json!("pc_tokens_ok"), json!("viewer_joined")]);
    assert_eq!(next_json(&mut viewer).await["t"], "pc_online");
}

#[tokio::test]
async fn a_pc_is_never_closed_for_frames_to_viewers_that_left() {
    let (addr, _state) = spawn().await;
    let (mut pc, pc_id) = pc_hello(addr, "churned", &["c"]).await;
    let (_viewer, _) = attached_viewer(addr, &mut pc, pc_id, "c").await;
    for _ in 0..200 {
        pc.send(frame(Uuid::new_v4(), 8)).await.unwrap();
    }
    assert_alive(&mut pc).await;
}

#[tokio::test]
async fn squatting_subscribers_cannot_keep_a_device_from_hearing_its_pc_come_online() {
    let (addr, _state) = spawn().await;
    let secret = "squatted";
    let pc_id = pc_id_for_secret_hash(&sha256_hex(secret));
    let mut squatters = Vec::new();
    for _ in 0..300 {
        let mut ws = connect(addr).await;
        subscribe(&mut ws, vec![sub_item(pc_id, Uuid::new_v4(), "junk")]).await;
        assert_eq!(next_json(&mut ws).await["t"], "pc_offline");
        squatters.push(ws);
    }
    let mut owner = connect(addr).await;
    subscribe(&mut owner, vec![sub_item(pc_id, Uuid::new_v4(), "real")]).await;
    assert_eq!(next_json(&mut owner).await["t"], "pc_offline");

    let (_pc, _) = pc_hello(addr, secret, &["real"]).await;
    assert_eq!(next_json(&mut owner).await, json!({ "t": "pc_online", "pcId": pc_id }));
}

async fn try_connect_from(addr: SocketAddr, ip: &str) -> Result<Ws, u16> {
    let mut req = url(addr).into_client_request().unwrap();
    req.headers_mut().insert("X-Real-IP", ip.parse().unwrap());
    match tokio_tungstenite::connect_async(req).await {
        Ok((ws, _)) => Ok(ws),
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => Err(resp.status().as_u16()),
        Err(other) => panic!("connect failed: {other}"),
    }
}

async fn connect_from(addr: SocketAddr, ip: &str) -> Ws {
    try_connect_from(addr, ip).await.unwrap()
}

async fn pc_hello_on(mut ws: Ws, secret: &str, room_tokens: &[&str]) -> (Ws, Uuid) {
    let secret_hash = sha256_hex(secret);
    let pc_id = pc_id_for_secret_hash(&secret_hash);
    let tokens: Vec<String> = room_tokens.iter().map(|t| sha256_hex(t)).collect();
    send(&mut ws, json!({ "t": "pc_hello", "pcId": pc_id, "pcSecretHash": secret_hash, "allowedTokens": tokens, "proto": 2 })).await;
    assert_eq!(next_json(&mut ws).await["t"], "pc_ready");
    (ws, pc_id)
}

async fn attach_on(mut viewer: Ws, pc: &mut Ws, pc_id: Uuid, token: &str) -> (Ws, Uuid) {
    subscribe(&mut viewer, vec![sub_item(pc_id, Uuid::new_v4(), token)]).await;
    assert_eq!(next_json(&mut viewer).await["t"], "pc_online");
    let joined = next_json(pc).await;
    assert_eq!(joined["t"], "viewer_joined");
    (viewer, Uuid::parse_str(joined["viewerId"].as_str().unwrap()).unwrap())
}

async fn next_binary(ws: &mut Ws) -> Vec<u8> {
    loop {
        match tokio::time::timeout(WAIT, ws.next()).await.expect("timed out waiting for a frame").unwrap().unwrap() {
            TMsg::Binary(b) => return b.to_vec(),
            TMsg::Ping(_) | TMsg::Pong(_) => continue,
            other => panic!("expected a frame, got {other:?}"),
        }
    }
}

async fn json_until(ws: &mut Ws, t: &str) {
    while next_json(ws).await["t"] != t {}
}

#[tokio::test]
async fn a_flood_from_many_prefixes_sheds_the_flooders_not_an_honest_stream() {
    let unlimited = RateLimit { bytes_per_sec: 1e12, burst_bytes: 1e12, msgs_per_sec: 1e9, burst_msgs: 1e9 };
    let budget = 4 * 1024 * 1024;
    let config = RelayConfig {
        global_queue_budget: budget,
        write_timeout: Duration::from_secs(60),
        queue_age_limit: Duration::from_secs(60),
        pc_rate: unlimited,
        ip_rate: unlimited,
        ..roomy()
    };
    let (addr, state) = spawn_with(config).await;

    let mut flooders = Vec::new();
    let mut stalled = Vec::new();
    for net in 0..4 {
        let pc_ws = connect_from(addr, &format!("2001:db8:{net:x}::1")).await;
        let (mut pc, pc_id) = pc_hello_on(pc_ws, &format!("flooder-{net}"), &["a", "b"]).await;
        let mut targets = Vec::new();
        for (sub, token) in ["a", "b"].into_iter().enumerate() {
            let viewer_ws = connect_from(addr, &format!("2001:db8:{net:x}:{sub:x}::2")).await;
            let (viewer, viewer_id) = attach_on(viewer_ws, &mut pc, pc_id, token).await;
            stalled.push(viewer);
            targets.push(viewer_id);
        }
        flooders.push((pc, targets));
    }
    let (mut honest_pc, honest_pc_id) = pc_hello_on(connect_from(addr, "198.51.100.7").await, "honest", &["h"]).await;
    let (mut honest, honest_id) = attach_on(connect_from(addr, "198.51.100.8").await, &mut honest_pc, honest_pc_id, "h").await;

    let mut rounds = 0;
    while state.queued_bytes() < budget - 512 * 1024 {
        rounds += 1;
        assert!(rounds < 400, "the flood never filled the relay ({} bytes queued)", state.queued_bytes());
        for (pc, targets) in &mut flooders {
            for target in targets.iter() {
                pc.send(frame(*target, 200 * 1024)).await.unwrap();
            }
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    let mut peak = 0;
    for seq in 0..40u32 {
        for (pc, targets) in &mut flooders {
            for target in targets.iter() {
                pc.send(frame(*target, 200 * 1024)).await.unwrap();
            }
        }
        let mut f = vec![BIN_VERSION];
        f.extend_from_slice(honest_id.as_bytes());
        f.extend_from_slice(&seq.to_be_bytes());
        f.resize(17 + 64 * 1024, 0xCD);
        honest_pc.send(TMsg::Binary(f.into())).await.unwrap();
        let got = next_binary(&mut honest).await;
        assert_eq!(&got[17..21], &seq.to_be_bytes(), "the honest viewer lost a frame");
        peak = peak.max(state.queued_bytes());
    }
    assert!(peak <= budget + 512 * 1024, "queued {peak} bytes with a {budget} byte budget");
    drop(stalled);
}

#[tokio::test]
async fn connection_caps_nest_so_neighbours_are_not_locked_out() {
    let unlimited = RateLimit { bytes_per_sec: f64::MAX, burst_bytes: f64::MAX, msgs_per_sec: 1e9, burst_msgs: 1e9 };
    let config = RelayConfig {
        max_connections_per_ip: 2,
        max_connections_per_site: 4,
        max_connections_per_v6_32: 6,
        connect_rate: unlimited,
        ..RelayConfig::default()
    };
    let (addr, _state) = spawn_with(config).await;
    let mut open = Vec::new();
    for (ip, admitted) in [
        ("2001:db8:1:1::1", true),
        ("2001:db8:1:1::2", true),
        ("2001:db8:1:1::3", false),
        ("2001:db8:1:2::1", true),
        ("2001:db8:1:2::1", true),
        ("2001:db8:1:3::1", true),
        ("2001:db8:1:3::1", false),
        ("2001:db8:2:1::1", true),
        ("2001:db8:2:1::1", true),
        ("2001:db8:3:1::1", true),
        ("2001:db8:3:2::1", false),
        ("2001:db9:1:1::1", true),
        ("198.51.100.1", true),
    ] {
        match try_connect_from(addr, ip).await {
            Ok(ws) => {
                assert!(admitted, "{ip} should have been refused");
                open.push(ws);
            }
            Err(status) => {
                assert!(!admitted, "{ip} was refused with {status}");
                assert_eq!(status, 429);
            }
        }
    }
}

#[tokio::test]
async fn a_client_over_its_own_connect_rate_does_not_spend_its_neighbours() {
    let slow = RateLimit { bytes_per_sec: f64::MAX, burst_bytes: f64::MAX, msgs_per_sec: 0.001, burst_msgs: 2.0 };
    let (addr, _state) = spawn_with(RelayConfig { connect_rate: slow, ..RelayConfig::default() }).await;
    let mut open = Vec::new();
    for _ in 0..2 {
        open.push(connect_from(addr, "2001:db8:7:1::1").await);
    }
    for _ in 0..20 {
        assert_eq!(try_connect_from(addr, "2001:db8:7:1::1").await.err(), Some(429));
    }
    for _ in 0..2 {
        open.push(connect_from(addr, "2001:db8:7:2::1").await);
    }
}

#[tokio::test]
async fn a_refused_subscriber_waits_from_its_first_refusal_not_its_subscribe() {
    let stale = Duration::from_millis(300);
    let (addr, _state) = spawn_with(RelayConfig { stale_subscription: stale, ..roomy() }).await;
    let (_anchor, anchor_id) = pc_hello(addr, "anchor", &["a"]).await;
    let late_secret = "late";
    let late_id = pc_id_for_secret_hash(&sha256_hex(late_secret));
    let mut viewer = connect(addr).await;
    subscribe(&mut viewer, vec![sub_item(anchor_id, Uuid::new_v4(), "a"), sub_item(late_id, Uuid::new_v4(), "b")]).await;
    let mut first = [next_json(&mut viewer).await["t"].clone(), next_json(&mut viewer).await["t"].clone()];
    first.sort_by_key(|t| t.to_string());
    assert_eq!(first, [json!("pc_offline"), json!("pc_online")]);

    tokio::time::sleep(stale * 2).await;
    let (mut late, _) = pc_hello(addr, late_secret, &[]).await;
    send(&mut late, json!({ "t": "pc_tokens", "allowedTokens": [sha256_hex("b")] })).await;
    assert_eq!(next_json(&mut viewer).await, json!({ "t": "pc_online", "pcId": late_id }));
    json_until(&mut late, "pc_tokens_ok").await;

    send(&mut late, json!({ "t": "pc_tokens", "allowedTokens": [] })).await;
    assert_eq!(next_json(&mut viewer).await, json!({ "t": "pc_offline", "pcId": late_id }));
    json_until(&mut late, "pc_tokens_ok").await;
    send(&mut late, json!({ "t": "pc_tokens", "allowedTokens": [] })).await;
    json_until(&mut late, "pc_tokens_ok").await;
    tokio::time::sleep(stale * 2).await;
    send(&mut late, json!({ "t": "pc_tokens", "allowedTokens": [] })).await;
    json_until(&mut late, "pc_tokens_ok").await;
    send(&mut late, json!({ "t": "pc_tokens", "allowedTokens": [sha256_hex("b")] })).await;
    json_until(&mut late, "pc_tokens_ok").await;
    assert_alive(&mut viewer).await;
}

#[tokio::test]
async fn a_slot_freed_by_a_token_change_goes_to_a_waiting_subscriber() {
    let (addr, _state) = spawn().await;
    let tokens: Vec<String> = (0..17).map(|i| format!("t{i}")).collect();
    let token_refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
    let (mut pc, pc_id) = pc_hello(addr, "rotating", &token_refs).await;
    let mut viewers = Vec::new();
    for token in &token_refs[..16] {
        viewers.push(attached_viewer(addr, &mut pc, pc_id, token).await);
    }
    let mut waiting = connect(addr).await;
    subscribe(&mut waiting, vec![sub_item(pc_id, Uuid::new_v4(), token_refs[16])]).await;
    assert_eq!(next_json(&mut waiting).await["t"], "pc_err");

    let kept: Vec<String> = token_refs[1..].iter().map(|t| sha256_hex(t)).collect();
    send(&mut pc, json!({ "t": "pc_tokens", "allowedTokens": kept })).await;
    assert_eq!(next_json(&mut viewers[0].0).await, json!({ "t": "pc_offline", "pcId": pc_id }));
    assert_eq!(next_json(&mut waiting).await, json!({ "t": "pc_online", "pcId": pc_id }));
}

#[tokio::test]
async fn a_repeated_subscribe_announces_the_pc_again() {
    let (addr, _state) = spawn().await;
    let (mut pc, pc_id) = pc_hello(addr, "re-announce", &["r"]).await;
    let mut viewer = connect(addr).await;
    let item = sub_item(pc_id, Uuid::new_v4(), "r");
    subscribe(&mut viewer, vec![item.clone()]).await;
    assert_eq!(next_json(&mut viewer).await["t"], "pc_online");
    let joined = next_json(&mut pc).await;
    assert_eq!(joined["t"], "viewer_joined");

    subscribe(&mut viewer, vec![item.clone()]).await;
    assert_alive(&mut viewer).await;
    let mut restart = item;
    restart["restart"] = json!(true);
    subscribe(&mut viewer, vec![restart]).await;
    assert_eq!(next_json(&mut viewer).await, json!({ "t": "pc_online", "pcId": pc_id }));
    let again = next_json(&mut pc).await;
    assert_eq!(again["t"], "viewer_joined", "the PC starts a new session for the same socket");
    assert_eq!(again["viewerId"], joined["viewerId"]);
    assert_alive(&mut pc).await;
}

#[tokio::test]
async fn the_relay_wide_manual_code_budget_stops_guessers_but_not_other_networks() {
    let (addr, _state) = spawn().await;
    let (mut pc, _) = pc_hello(addr, "guessed", &[]).await;
    send(&mut pc, json!({ "t": "pair_open", "pairId": "00000000000000b1", "manualCode": "REAL-2345" })).await;
    assert_eq!(next_json(&mut pc).await["t"], "pair_opened");

    let per_ip = 30;
    let guessers = MANUAL_MISS_MAX as usize / per_ip;
    for g in 0..guessers {
        let mut ws = connect_from(addr, &format!("203.0.{g}.1")).await;
        for n in 0..per_ip {
            send(&mut ws, json!({ "t": "pair_join", "pairId": format!("GUES{n:04}"), "data": "x", "proto": 2 })).await;
        }
        for _ in 0..per_ip {
            assert_eq!(next_json(&mut ws).await, json!({ "t": "pair_err", "code": "not_found" }));
        }
        if g == 0 {
            send(&mut ws, json!({ "t": "pair_join", "pairId": "GUES9999", "data": "x", "proto": 2 })).await;
            assert_eq!(next_json(&mut ws).await, json!({ "t": "pair_err", "code": "rate_limited" }));
            expect_closed(&mut ws).await;
        }
    }

    let mut guesser_again = connect_from(addr, "203.0.3.2").await;
    send(&mut guesser_again, json!({ "t": "pair_join", "pairId": "REAL2345", "data": "x", "proto": 2 })).await;
    assert_eq!(next_json(&mut guesser_again).await, json!({ "t": "pair_err", "code": "rate_limited" }));
    expect_closed(&mut guesser_again).await;

    let mut right_code = connect_from(addr, "198.51.100.50").await;
    send(&mut right_code, json!({ "t": "pair_join", "pairId": "REAL2345", "data": "typed", "proto": 2 })).await;
    let joined = next_json(&mut pc).await;
    assert_eq!(joined["data"], "typed", "a network that guessed nothing still pairs by code");

    let mut qr = connect_from(addr, "198.51.100.51").await;
    send(&mut qr, json!({ "t": "pair_join", "pairId": "00000000000000b1", "data": "qr", "proto": 2 })).await;
    let joined = next_json(&mut pc).await;
    assert_eq!(joined["t"], "pair_joined");
    assert_eq!(joined["data"], "qr");
}

#[tokio::test]
async fn one_network_cannot_spend_the_whole_manual_code_budget() {
    let (addr, _state) = spawn().await;
    let (mut pc, _) = pc_hello(addr, "guessed-by-one", &[]).await;
    send(&mut pc, json!({ "t": "pair_open", "pairId": "00000000000000b2", "manualCode": "MINE-2345" })).await;
    assert_eq!(next_json(&mut pc).await["t"], "pair_opened");

    let share = (MANUAL_MISS_MAX / MANUAL_MISS_SHARE) as usize;
    let mut misses = 0;
    for host in 1..=3 {
        let mut ws = connect_from(addr, &format!("203.0.113.{host}")).await;
        for n in 0..20 {
            send(&mut ws, json!({ "t": "pair_join", "pairId": format!("GUES{host}{n:03}"), "data": "x", "proto": 2 })).await;
            let reply = next_json(&mut ws).await;
            if reply["code"] == "rate_limited" {
                break;
            }
            misses += 1;
        }
    }
    assert_eq!(misses, share);

    let mut owner = connect_from(addr, "198.51.100.60").await;
    send(&mut owner, json!({ "t": "pair_join", "pairId": "MINE2345", "data": "x", "proto": 2 })).await;
    assert_eq!(next_json(&mut pc).await["t"], "pair_joined");
}

#[tokio::test]
async fn the_per_address_join_limit_counts_pairings_not_joins() {
    let unlimited = RateLimit { bytes_per_sec: f64::MAX, burst_bytes: f64::MAX, msgs_per_sec: 1e9, burst_msgs: 1e9 };
    let (addr, _state) = spawn_with(RelayConfig { control_rate: unlimited, ..roomy() }).await;
    let (mut pc, _) = pc_hello(addr, "popular-pairer", &[]).await;
    let mut phones = Vec::new();
    for n in 0..=30 {
        let pair_id = format!("{:016x}", 0xc000 + n);
        let code = format!("CODE{n:04}");
        send(&mut pc, json!({ "t": "pair_open", "pairId": pair_id, "manualCode": code })).await;
        assert_eq!(next_json(&mut pc).await["t"], "pair_opened");
        let mut phone = connect(addr).await;
        send(&mut phone, json!({ "t": "pair_join", "pairId": code, "data": "commit", "proto": 2 })).await;
        if n == 30 {
            assert_eq!(next_json(&mut phone).await, json!({ "t": "pair_err", "code": "rate_limited" }));
            expect_closed(&mut phone).await;
            break;
        }
        assert_eq!(next_json(&mut pc).await["data"], "commit");
        for step in ["reveal", "done"] {
            send(&mut phone, json!({ "t": "pair_join", "pairId": pair_id, "data": step, "proto": 2 })).await;
            assert_eq!(next_json(&mut pc).await["data"], step);
        }
        phones.push(phone);
    }
}

#[tokio::test]
async fn a_pc_with_no_device_and_no_pairing_room_is_closed() {
    let (addr, _state) = spawn_with(RelayConfig { purposeless_timeout: Duration::from_millis(300), ..roomy() }).await;
    let (mut idle, _) = pc_hello(addr, "nothing-paired", &[]).await;
    let (mut paired, _) = pc_hello(addr, "paired", &["d"]).await;
    let (mut pairing, _) = pc_hello(addr, "pairing", &[]).await;
    send(&mut pairing, json!({ "t": "pair_open", "pairId": "00000000000000e2" })).await;
    assert_eq!(next_json(&mut pairing).await["t"], "pair_opened");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(next_json(&mut idle).await, json!({ "t": "err", "code": "no_purpose" }));
    expect_closed(&mut idle).await;
    assert_alive(&mut paired).await;
    assert_alive(&mut pairing).await;
}

#[tokio::test]
async fn freed_slots_fill_up_in_the_order_viewers_waited() {
    let (addr, _state) = spawn().await;
    let others: Vec<String> = (0..14).map(|i| format!("o{i}")).collect();
    let mut tokens: Vec<&str> = vec!["a", "b"];
    tokens.extend(others.iter().map(String::as_str));
    let (mut pc, pc_id) = pc_hello(addr, "queued-up", &tokens).await;
    let (mut a1, _) = attached_viewer(addr, &mut pc, pc_id, "a").await;
    let _a2 = attached_viewer(addr, &mut pc, pc_id, "a").await;
    let mut attached = Vec::new();
    for token in &others {
        attached.push(attached_viewer(addr, &mut pc, pc_id, token).await);
    }
    let mut waiters = Vec::new();
    for token in ["a", "a", "b"] {
        let mut ws = connect(addr).await;
        subscribe(&mut ws, vec![sub_item(pc_id, Uuid::new_v4(), token)]).await;
        assert_eq!(next_json(&mut ws).await, json!({ "t": "pc_err", "pcId": pc_id, "code": "full" }));
        waiters.push(ws);
    }
    drop(attached.remove(0));
    assert_eq!(next_json(&mut waiters[0]).await["t"], "pc_online", "the longest waiting goes first");
    drop(attached.remove(0));
    assert_eq!(next_json(&mut waiters[1]).await["t"], "pc_online");
    assert_eq!(next_json(&mut a1).await["code"], "replaced", "it replaced the oldest socket of its own device");
    assert_eq!(next_json(&mut waiters[2]).await["t"], "pc_online", "so the slot still went to the next in line");
}

#[tokio::test]
async fn oversized_pairing_payloads_are_refused() {
    let (addr, _state) = spawn().await;
    let (mut pc, _) = pc_hello(addr, "small-pairing", &[]).await;
    send(&mut pc, json!({ "t": "pair_open", "pairId": "00000000000000e3" })).await;
    assert_eq!(next_json(&mut pc).await["t"], "pair_opened");
    let mut phone = connect(addr).await;
    send(&mut phone, json!({ "t": "pair_join", "pairId": "00000000000000e3", "data": "x".repeat(9 * 1024), "proto": 2 })).await;
    assert_eq!(next_json(&mut phone).await["code"], "frame_too_large");
    send(&mut phone, json!({ "t": "pair_join", "pairId": "00000000000000e3", "data": "ok", "proto": 2 })).await;
    let joined = next_json(&mut pc).await;
    assert_eq!(joined["data"], "ok");
    send(&mut pc, json!({ "t": "pair_msg", "pairId": "00000000000000e3", "viewerId": joined["viewerId"], "data": "y".repeat(9 * 1024) })).await;
    assert_eq!(next_json(&mut pc).await["code"], "frame_too_large");
}

#[tokio::test]
async fn a_home_with_sixteen_pcs_and_its_dashboards_shares_one_address() {
    let (addr, state) = spawn_with(RelayConfig::default()).await;
    let mut home = Vec::new();
    for _ in 0..20 {
        home.push(connect_from(addr, "203.0.113.20").await);
    }
    assert_eq!(state.connections(), 20);
}

#[tokio::test]
async fn after_a_router_blip_a_home_reconnects_past_its_dead_sockets() {
    let ping = Duration::from_millis(200);
    let (addr, _state) = spawn_with(RelayConfig { max_connections_per_ip: 4, ping_interval: ping, ..roomy() }).await;
    let mut dead = Vec::new();
    for _ in 0..4 {
        dead.push(connect_from(addr, "203.0.113.30").await);
    }
    assert_eq!(try_connect_from(addr, "203.0.113.30").await.err(), Some(429), "every socket of the home is live");
    tokio::time::sleep(ping * 2).await;
    let mut back = Vec::new();
    for _ in 0..4 {
        back.push(connect_from(addr, "203.0.113.30").await);
    }
    for ws in &mut dead {
        expect_closed(ws).await;
    }
    for ws in &mut back {
        assert_alive(ws).await;
    }
}

#[tokio::test]
async fn a_refused_subscriber_that_keeps_resubscribing_still_stops_waiting() {
    let stale = Duration::from_millis(300);
    let (addr, _state) = spawn_with(RelayConfig { stale_subscription: stale, ..roomy() }).await;
    let (mut pc, pc_id) = pc_hello(addr, "revoker", &["good"]).await;
    let mut viewer = connect(addr).await;
    for _ in 0..3 {
        subscribe(&mut viewer, vec![sub_item(pc_id, Uuid::new_v4(), "revoked")]).await;
        assert_eq!(next_json(&mut viewer).await["t"], "pc_offline");
        tokio::time::sleep(stale * 2 / 3).await;
    }
    send(&mut pc, json!({ "t": "pc_tokens", "allowedTokens": [sha256_hex("good"), sha256_hex("revoked")] })).await;
    json_until(&mut pc, "pc_tokens_ok").await;
    assert_alive(&mut viewer).await;
}

#[tokio::test]
async fn a_refused_subscriber_that_changes_its_token_every_time_still_stops_waiting() {
    let stale = Duration::from_millis(300);
    let (addr, _state) = spawn_with(RelayConfig { stale_subscription: stale, ..roomy() }).await;
    let (mut pc, pc_id) = pc_hello(addr, "rotated", &["good"]).await;
    let mut viewer = connect(addr).await;
    for i in 0..3 {
        subscribe(&mut viewer, vec![sub_item(pc_id, Uuid::new_v4(), &format!("guess-{i}"))]).await;
        assert_eq!(next_json(&mut viewer).await["t"], "pc_offline");
        tokio::time::sleep(stale * 2 / 3).await;
    }
    send(&mut pc, json!({ "t": "pc_tokens", "allowedTokens": [sha256_hex("good"), sha256_hex("guess-2")] })).await;
    json_until(&mut pc, "pc_tokens_ok").await;
    assert_alive(&mut viewer).await;
}

#[tokio::test]
async fn a_pc_coming_online_attaches_the_longest_waiting_subscribers_first() {
    let (addr, _state) = spawn().await;
    let secret = "crowded-at-boot";
    let pc_id = pc_id_for_secret_hash(&sha256_hex(secret));
    let tokens: Vec<String> = (0..17).map(|i| format!("w{i}")).collect();
    let mut waiting = Vec::new();
    for token in &tokens {
        let mut ws = connect(addr).await;
        subscribe(&mut ws, vec![sub_item(pc_id, Uuid::new_v4(), token)]).await;
        assert_eq!(next_json(&mut ws).await["t"], "pc_offline");
        waiting.push(ws);
    }
    let token_refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
    let (_pc, _) = pc_hello(addr, secret, &token_refs).await;
    for ws in &mut waiting[..16] {
        assert_eq!(next_json(ws).await["t"], "pc_online");
    }
    assert_alive(&mut waiting[16]).await;
}

#[tokio::test]
async fn pairing_messages_get_through_after_the_pc_spends_its_control_budget() {
    let (addr, _state) = spawn().await;
    let (mut pc, _) = pc_hello(addr, "busy-pairer", &[]).await;
    send(&mut pc, json!({ "t": "pair_open", "pairId": "00000000000000f9" })).await;
    assert_eq!(next_json(&mut pc).await["t"], "pair_opened");
    let mut phone = connect(addr).await;
    send(&mut phone, json!({ "t": "pair_join", "pairId": "00000000000000f9", "data": "hi", "proto": 2 })).await;
    let viewer_id = next_json(&mut pc).await["viewerId"].clone();
    for _ in 0..12 {
        send(&mut pc, json!({ "t": "pc_tokens", "allowedTokens": [] })).await;
    }
    send(&mut pc, json!({ "t": "pair_extend", "pairId": "00000000000000f9" })).await;
    send(&mut pc, json!({ "t": "pair_msg", "pairId": "00000000000000f9", "viewerId": viewer_id, "data": "ack" })).await;
    assert_eq!(next_json(&mut phone).await, json!({ "t": "pair_msg", "pairId": "00000000000000f9", "data": "ack" }));
}

#[tokio::test]
async fn a_viewer_following_many_busy_pcs_still_hears_a_quiet_one() {
    let unlimited = RateLimit { bytes_per_sec: 1e12, burst_bytes: 1e12, msgs_per_sec: 1e9, burst_msgs: 1e9 };
    let config = RelayConfig {
        write_timeout: Duration::from_secs(60),
        queue_age_limit: Duration::from_secs(60),
        pc_rate: unlimited,
        ip_rate: unlimited,
        ..roomy()
    };
    let (addr, state) = spawn_with(config).await;
    let mut pcs = Vec::new();
    for n in 0..9 {
        pcs.push(pc_hello(addr, &format!("farm-{n}"), &["phone"]).await);
    }
    let mut viewer = connect(addr).await;
    let device = Uuid::new_v4();
    subscribe(&mut viewer, pcs.iter().map(|(_, pc_id)| sub_item(*pc_id, device, "phone")).collect()).await;
    let mut viewer_id = None;
    for (pc, _) in &mut pcs {
        let joined = next_json(pc).await;
        assert_eq!(joined["t"], "viewer_joined");
        viewer_id = Some(Uuid::parse_str(joined["viewerId"].as_str().unwrap()).unwrap());
    }
    let viewer_id = viewer_id.unwrap();
    let (quiet, busy) = pcs.split_last_mut().unwrap();
    for _ in 0..64 {
        for (pc, _) in busy.iter_mut() {
            pc.send(frame(viewer_id, 200 * 1024)).await.unwrap();
        }
    }
    for (pc, _) in busy.iter_mut() {
        assert_alive(pc).await;
    }
    let mut waited = 0;
    while state.queued_bytes() < 2 * 1024 * 1024 - 256 * 1024 {
        waited += 1;
        assert!(waited < 200, "the busy PCs never filled the viewer queue ({} bytes)", state.queued_bytes());
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut marked = vec![BIN_VERSION];
    marked.extend_from_slice(viewer_id.as_bytes());
    marked.resize(17 + 200 * 1024, 0x51);
    quiet.0.send(TMsg::Binary(marked.into())).await.unwrap();
    assert_alive(&mut quiet.0).await;
    for _ in 0..1000 {
        let next = tokio::time::timeout(WAIT, viewer.next()).await.expect("the viewer stopped hearing anything");
        if let Some(Ok(TMsg::Binary(got))) = next
            && got[1..17] == *quiet.1.as_bytes()
        {
            assert_eq!(got.len(), 17 + 200 * 1024);
            return;
        }
    }
    panic!("the frame of the quiet PC never arrived");
}

fn forgetful() -> RelayConfig {
    RelayConfig { purposeless_timeout: Duration::from_millis(300), ping_interval: Duration::from_millis(100), ..roomy() }
}

#[tokio::test]
async fn a_viewer_waiting_on_a_full_pc_that_leaves_is_closed() {
    let (addr, _state) = spawn_with(forgetful()).await;
    let tokens: Vec<String> = (0..17).map(|i| format!("w{i}")).collect();
    let token_refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
    let (mut pc, pc_id) = pc_hello(addr, "crowded-then-gone", &token_refs).await;
    let mut viewers = Vec::new();
    for token in &token_refs[..16] {
        viewers.push(attached_viewer(addr, &mut pc, pc_id, token).await);
    }
    let mut waiting = connect(addr).await;
    subscribe(&mut waiting, vec![sub_item(pc_id, Uuid::new_v4(), token_refs[16])]).await;
    assert_eq!(next_json(&mut waiting).await["code"], "full");
    drop(pc);
    closes_while_pinging(&mut waiting).await;
}

#[tokio::test]
async fn a_viewer_whose_pc_registers_again_without_its_token_is_closed() {
    let (addr, _state) = spawn_with(forgetful()).await;
    let secret = "re-registered";
    let (mut pc, pc_id) = pc_hello(addr, secret, &["a"]).await;
    let (mut viewer, _) = attached_viewer(addr, &mut pc, pc_id, "a").await;
    let (pc2, _) = pc_hello(addr, secret, &[]).await;
    drop(pc2);
    drop(pc);
    closes_while_pinging(&mut viewer).await;
}

#[tokio::test]
async fn a_viewer_that_unsubscribes_from_its_only_pc_is_closed() {
    let config = RelayConfig { purposeless_timeout: Duration::from_millis(300), ..roomy() };
    let (addr, _state) = spawn_with(config).await;
    let (mut pc, pc_id) = pc_hello(addr, "left-behind", &["u"]).await;
    let (mut viewer, _) = attached_viewer(addr, &mut pc, pc_id, "u").await;
    send(&mut viewer, json!({ "t": "unsubscribe", "pcId": pc_id })).await;
    closes_while_pinging(&mut viewer).await;
}

#[tokio::test]
async fn a_refused_pc_left_out_of_a_subscribe_and_added_back_keeps_its_refusal_clock() {
    let stale = Duration::from_millis(300);
    let (addr, _state) = spawn_with(RelayConfig { stale_subscription: stale, ..roomy() }).await;
    let (_own, own_id) = pc_hello(addr, "own-pc", &["mine"]).await;
    let (mut target, target_id) = pc_hello(addr, "target-pc", &["good"]).await;
    let mine = sub_item(own_id, Uuid::new_v4(), "mine");
    let guess = sub_item(target_id, Uuid::new_v4(), "guess");
    let mut viewer = connect(addr).await;
    subscribe(&mut viewer, vec![mine.clone(), guess.clone()]).await;
    json_until(&mut viewer, "pc_offline").await;
    for _ in 0..3 {
        tokio::time::sleep(stale * 2 / 3).await;
        subscribe(&mut viewer, vec![mine.clone()]).await;
        subscribe(&mut viewer, vec![mine.clone(), guess.clone()]).await;
        json_until(&mut viewer, "pc_offline").await;
    }
    send(&mut target, json!({ "t": "pc_tokens", "allowedTokens": [sha256_hex("good"), sha256_hex("guess")] })).await;
    json_until(&mut target, "pc_tokens_ok").await;
    assert_alive(&mut viewer).await;
}
