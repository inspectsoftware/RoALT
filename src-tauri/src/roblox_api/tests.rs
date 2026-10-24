use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;

use reqwest::header::HeaderName;

use super::*;

fn fixture(name: &str) -> String {
    let path = format!("{}/../docs/research/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).unwrap()
}

/// The recorded `body` of a JSON fixture, as it came over the wire.
fn fixture_body(name: &str) -> Vec<u8> {
    let v: Value = serde_json::from_str(&fixture(name)).unwrap();
    serde_json::to_vec(&v["body"]).unwrap()
}

/// A `.headers.txt` capture (status line, then `name: value` lines).
fn fixture_headers(name: &str) -> HeaderMap {
    headers(&fixture(name).lines().filter_map(|l| l.split_once(": ")).collect::<Vec<_>>())
}

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.append(HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
    }
    h
}

#[test]
fn rate_limit_headers() {
    let ok = RateLimit::parse(&fixture_headers("acctinfo-users-authenticated-nocookie.headers.txt"));
    assert_eq!(ok, RateLimit { window: Some(60), remaining: Some(99), reset: Some(32), retry_after: None });
    let limited = RateLimit::parse(&fixture_headers("acctinfo-ratelimit-429-friends-list.headers.txt"));
    assert_eq!(limited, RateLimit { window: Some(60), remaining: Some(0), reset: Some(30), retry_after: Some(5) });
    let servers = RateLimit::parse(&fixture_headers("servers-v1-429.headers.txt"));
    assert_eq!((servers.remaining, servers.reset_secs()), (Some(0), Some(50)));

    // No reset header: the next clock-aligned window boundary.
    let hourly = RateLimit::parse(&headers(&[("x-ratelimit-limit", "750, 750;w=3600"), ("x-ratelimit-remaining", "749")]));
    assert_eq!(hourly.window, Some(3600));
    assert!((1..=3600).contains(&hourly.reset_secs().unwrap()));

    assert_eq!(RateLimit::parse(&headers(&[("x-ratelimit-limit", "5, 5;w=1, 100;w=60")])).window, Some(1));
    assert_eq!(RateLimit::parse(&headers(&[("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT")])).retry_after, None);
    assert_eq!(RateLimit::parse(&HeaderMap::new()), RateLimit::default());
    assert_eq!(RateLimit::default().reset_secs(), None);
}

#[test]
fn set_cookie_rotation() {
    let new = ".ROBLOSECURITY=_|WARNING:-DO-NOT-SHARE-THIS.--x|_NEW; domain=.roblox.com; \
               expires=Mon, 24-Oct-2061 10:00:00 GMT; path=/; secure; HttpOnly";
    let h = headers(&[
        ("set-cookie", "__cf_bm=abc; HttpOnly; Path=/; Domain=roblox.com"),
        ("set-cookie", ".ROBLOSECURITY=; domain=.roblox.com; expires=Thu, 01-Jan-1970 00:00:00 GMT; path=/"),
        ("set-cookie", new),
        ("set-cookie", "rbx-ip2=1; domain=roblox.com; path=/"),
    ]);
    assert_eq!(rotated_cookie(&h).unwrap().expose(), "_|WARNING:-DO-NOT-SHARE-THIS.--x|_NEW");
    assert_eq!(rotated_cookie(&headers(&[("set-cookie", ".ROBLOSECURITY=PLAIN")])).unwrap().expose(), "PLAIN");

    for gone in [
        ".ROBLOSECURITY=; path=/",
        ".ROBLOSECURITY=deleted; Max-Age=0",
        ".ROBLOSECURITY=old; expires=Thu, 01 Jan 1970 00:00:00 GMT",
        "x.ROBLOSECURITY=other",
        "GuestData=UserID=-1; domain=.roblox.com",
    ] {
        assert!(rotated_cookie(&headers(&[("set-cookie", gone)])).is_none(), "{gone}");
    }
    assert!(rotated_cookie(&HeaderMap::new()).is_none());

    // The site cookies Roblox sets on an anonymous www request are no rotation.
    let v: Value = serde_json::from_str(&fixture("authlaunch-www-xsrftoken-get.json")).unwrap();
    let set: Vec<(&str, &str)> =
        v["headers"]["set-cookie"].as_array().unwrap().iter().map(|c| ("set-cookie", c.as_str().unwrap())).collect();
    assert_eq!(set.len(), 3);
    assert!(rotated_cookie(&headers(&set)).is_none());
}

#[test]
fn error_classification() {
    let none = HeaderMap::new();
    let missing = fixture_body("acctinfo-users-authenticated-nocookie.json");
    assert_eq!(error_code(&missing), Some((9002, 0)));
    assert!(matches!(classify(StatusCode::UNAUTHORIZED, &none, &missing), AppError::CookieInvalid));
    let ticket = fixture_body("authlaunch-auth-ticket-nocookie-nocsrf.json");
    assert!(matches!(classify(StatusCode::UNAUTHORIZED, &none, &ticket), AppError::CookieInvalid));

    let bound = br#"{"errors":[{"code":9002,"subcode":2,"message":"Unauthorized"}]}"#;
    assert!(matches!(classify(StatusCode::UNAUTHORIZED, &none, bound), AppError::BoundSessionRequired));

    // Bare string, {Error: ..} and empty bodies: no code, still a plain 401.
    let bare = fixture_body("links-sharelinks-resolve-server-unauth-401.json");
    for body in [&bare[..], br#"{"Error":"Unauthorized"}"#, b"", b"<html>"] {
        assert_eq!(error_code(body), None);
        assert!(matches!(classify(StatusCode::UNAUTHORIZED, &none, body), AppError::CookieInvalid));
    }

    // auth.roblox.com's flat CSRF body; a 403 that reaches classify is a plain error.
    let csrf = fixture_body("authlaunch-csrf-login-v2-nocsrf.json");
    assert_eq!(error_code(&csrf), Some((0, 0)));
    assert!(matches!(classify(StatusCode::FORBIDDEN, &none, &csrf), AppError::Roblox(m) if m == "HTTP 403"));
    let challenge = headers(&[("rblx-challenge-id", "abc"), ("rblx-challenge-type", "captcha")]);
    assert!(matches!(classify(StatusCode::FORBIDDEN, &challenge, b"{}"), AppError::Challenge));

    let gone = fixture_body("acctinfo-presence-last-online-nocookie.json");
    assert!(matches!(classify(StatusCode::NOT_FOUND, &none, &gone), AppError::Roblox(m) if m == "HTTP 404"));
}

// End to end against a canned HTTP server on 127.0.0.1.

const USER: &str = r#"{"id":1,"name":"roblox","displayName":"Roblox","hasVerifiedBadge":true}"#;
const TICKET: &str = "https://auth.roblox.com/v1/authentication-ticket";

fn reply(status: &str, headers: &[&str], body: &str) -> String {
    let extra: String = headers.iter().map(|h| format!("{h}\r\n")).collect();
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n{extra}\r\n{body}",
        body.len()
    )
}

/// Answers one connection per response, in order, and passes on each raw request.
fn serve(responses: Vec<String>) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for resp in responses {
            let Ok((mut s, _)) = listener.accept() else { return };
            let _ = tx.send(read_request(&mut s));
            let _ = s.write_all(resp.as_bytes());
        }
    });
    (base, rx)
}

fn read_request(s: &mut TcpStream) -> String {
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let (mut buf, mut chunk) = (Vec::new(), [0; 4096]);
    while let Ok(n @ 1..) = s.read(&mut chunk) {
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).into_owned();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let len = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap()))
                .unwrap_or(0);
            if body.len() >= len {
                return text;
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Case-insensitive "request contains this header line".
fn has(req: &str, line: &str) -> bool {
    req.to_ascii_lowercase().contains(&line.to_ascii_lowercase())
}

type Rotations = Arc<Mutex<Vec<(i64, String)>>>;

fn client(base: &str) -> (Client, Rotations) {
    let seen = Rotations::default();
    let sink = seen.clone();
    let on_rotate: RotationSink = Arc::new(move |id, c: Secret| sink.lock().unwrap().push((id, c.expose().to_owned())));
    (Client::with_base(on_rotate, Some(base.into())), seen)
}

#[tokio::test]
async fn authenticated_user_success() {
    let (base, reqs) = serve(vec![reply("200 OK", &[], USER)]);
    let (c, rotations) = client(&base);
    let (user, rotated) = c.authenticated_user(5, &Secret::new("OLD")).await.unwrap();
    assert_eq!(user, AuthenticatedUser { id: 1, name: "roblox".into(), display_name: "Roblox".into() });
    assert!(rotated.is_none() && rotations.lock().unwrap().is_empty());

    let req = reqs.recv().unwrap();
    assert!(req.starts_with("GET /v1/users/authenticated HTTP/1.1\r\n"), "{req}");
    assert!(has(&req, "\r\ncookie: .ROBLOSECURITY=OLD\r\n"));
    assert!(has(&req, "\r\nuser-agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64)"));
    // GET on users.roblox.com: no CSRF token, no website Origin.
    assert!(!has(&req, "x-csrf-token") && !has(&req, "\r\norigin:"));
}

#[tokio::test]
async fn csrf_token_is_stored_and_retried_once() {
    let csrf_403 = reply(
        "403 Forbidden",
        &["x-csrf-token: TOK1", "set-cookie: .ROBLOSECURITY=NEW; domain=.roblox.com; path=/"],
        r#"{"code":0,"message":"Token Validation Failed"}"#,
    );
    let (base, reqs) = serve(vec![csrf_403, reply("200 OK", &[], "{}"), reply("200 OK", &[], "{}")]);
    let (c, _) = client(&base);
    let (old, body) = (Secret::new("OLD"), serde_json::json!({}));

    let (_, _, rotated) = c.request(Method::POST, "ticket", TICKET, Some(&body), 3, &old).await.unwrap();
    assert_eq!(rotated.unwrap().expose(), "NEW");
    let (first, retry) = (reqs.recv().unwrap(), reqs.recv().unwrap());
    assert!(!has(&first, "x-csrf-token"));
    assert!(has(&retry, "\r\nx-csrf-token: TOK1\r\n"));
    // The retry already carries the cookie Roblox rotated in the 403.
    assert!(has(&retry, "\r\ncookie: .ROBLOSECURITY=NEW\r\n"));
    for r in [&first, &retry] {
        assert!(r.starts_with("POST /v1/authentication-ticket HTTP/1.1\r\n"));
        assert!(has(r, "\r\nreferer: https://www.roblox.com/\r\n") && has(r, "\r\norigin: https://www.roblox.com\r\n"));
        assert!(has(r, "\r\ncontent-type: application/json\r\n") && r.ends_with("\r\n\r\n{}"));
    }

    // The cached token goes out with the account's next call straight away.
    c.request(Method::POST, "ticket", TICKET, Some(&body), 3, &old).await.unwrap();
    assert!(has(&reqs.recv().unwrap(), "\r\nx-csrf-token: TOK1\r\n"));
}

#[tokio::test]
async fn csrf_retry_happens_only_once() {
    let csrf_403 = reply("403 Forbidden", &["x-csrf-token: TOK"], "{}");
    let (base, _reqs) = serve(vec![csrf_403.clone(), csrf_403]);
    let (c, _) = client(&base);
    let r = c.request(Method::POST, "ticket", TICKET, None, 3, &Secret::new("OLD")).await;
    assert!(matches!(r, Err(AppError::Roblox(m)) if m == "HTTP 403"));
}

#[tokio::test]
async fn rotated_cookie_reaches_sink_and_caller() {
    let set = [
        "set-cookie: __cf_bm=x; Path=/; Domain=roblox.com",
        "set-cookie: .ROBLOSECURITY=NEW; domain=.roblox.com; expires=Mon, 24-Oct-2061 10:00:00 GMT; path=/; secure; HttpOnly",
    ];
    let ok = reply("200 OK", &set, USER);
    let (base, _reqs) = serve(vec![ok.clone(), ok.clone(), ok]);
    let (c, rotations) = client(&base);

    let (_, rotated) = c.authenticated_user(7, &Secret::new("OLD")).await.unwrap();
    assert_eq!(rotated.unwrap().expose(), "NEW");
    assert_eq!(*rotations.lock().unwrap(), vec![(7, "NEW".to_string())]);

    // A cookie that is not stored yet (id 0) comes back but never reaches the sink.
    let (_, rotated) = c.authenticated_user(0, &Secret::new("OLD")).await.unwrap();
    assert_eq!(rotated.unwrap().expose(), "NEW");
    assert_eq!(rotations.lock().unwrap().len(), 1);

    // Roblox repeating the current value is no rotation.
    let (_, rotated) = c.authenticated_user(7, &Secret::new("NEW")).await.unwrap();
    assert!(rotated.is_none());
    assert_eq!(rotations.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn rate_limited_then_success() {
    let limited = reply(
        "429 Too Many Requests",
        &["x-ratelimit-limit: 20, 20;w=60", "x-ratelimit-remaining: 0", "x-ratelimit-reset: 1", "retry-after: 5"],
        r#"{"errors":[{"code":0,"message":""}]}"#,
    );
    let (base, _reqs) = serve(vec![limited, reply("200 OK", &[], USER)]);
    let (c, _) = client(&base);
    let start = Instant::now();
    let (user, _) = c.authenticated_user(1, &Secret::new("OLD")).await.unwrap();
    assert_eq!(user.id, 1);
    // Waited for x-ratelimit-reset (1 s), not retry-after (5 s).
    assert!((Duration::from_millis(900)..Duration::from_secs(4)).contains(&start.elapsed()));
}

#[tokio::test]
async fn rate_limit_gives_up_after_three_tries() {
    let limited = reply("429 Too Many Requests", &["x-ratelimit-reset: 1"], "");
    let (base, _reqs) = serve(vec![limited.clone(), limited.clone(), limited]);
    let (c, _) = client(&base);
    assert!(matches!(c.authenticated_user(1, &Secret::new("OLD")).await, Err(AppError::RateLimited(1))));
}

#[tokio::test]
async fn unauthorized_is_cookie_invalid_or_bound_session() {
    let missing = String::from_utf8(fixture_body("acctinfo-users-authenticated-nocookie.json")).unwrap();
    let bound = r#"{"errors":[{"code":9002,"subcode":2,"message":"Unauthorized"}]}"#;
    let (base, _reqs) = serve(vec![reply("401 Unauthorized", &[], &missing), reply("401 Unauthorized", &[], bound)]);
    let (c, _) = client(&base);
    let cookie = Secret::new("OLD");
    assert!(matches!(c.authenticated_user(1, &cookie).await, Err(AppError::CookieInvalid)));
    assert!(matches!(c.authenticated_user(1, &cookie).await, Err(AppError::BoundSessionRequired)));
}

#[tokio::test]
async fn challenge_is_not_retried() {
    let challenge = reply(
        "403 Forbidden",
        &["rblx-challenge-id: abc", "rblx-challenge-type: captcha", "rblx-challenge-metadata: e30=", "x-csrf-token: TOK"],
        "{}",
    );
    // One canned answer: a retry would hit a closed port and fail as Network.
    let (base, _reqs) = serve(vec![challenge]);
    let (c, _) = client(&base);
    let url = "https://apis.roblox.com/sharelinks/v1/resolve-link";
    let r = c.request(Method::POST, "share", url, None, 1, &Secret::new("OLD")).await;
    assert!(matches!(r, Err(AppError::Challenge)));
}

#[tokio::test]
async fn network_error_names_host_only() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let (c, _) = client(&format!("http://127.0.0.1:{port}"));
    match c.authenticated_user(1, &Secret::new("OLD")).await {
        Err(AppError::Network(m)) => assert!(!m.contains("/v1") && !m.contains("OLD"), "{m}"),
        r => panic!("{r:?}"),
    }
}

#[tokio::test]
async fn used_up_window_delays_the_next_call() {
    let (c, _) = client("http://127.0.0.1:1");
    c.set_window("route", 0, 1);
    let start = Instant::now();
    c.wait_turn("route").await;
    assert!(start.elapsed() >= Duration::from_millis(900));

    c.set_window("route", 1, 60);
    let start = Instant::now();
    c.wait_turn("route").await;
    assert!(start.elapsed() < Duration::from_millis(500));
    assert_eq!(lock(&c.windows)["route"].remaining, 0);
}
