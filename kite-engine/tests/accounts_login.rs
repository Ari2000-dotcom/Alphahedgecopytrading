//! The accounts store and automatic Zerodha sign-in, against a local mock of
//! kite.zerodha.com and api.kite.trade. No real broker is contacted.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use kite_engine::accounts::{Accounts, Upsert};
use kite_engine::login::BrokerUrls;
use kite_engine::totp;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

const TOTP_SECRET: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

type Calls = Arc<Mutex<Vec<String>>>;

async fn mock(State(calls): State<Calls>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let form: HashMap<String, String> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    calls.lock().unwrap().push(format!("{method} {}", uri.path()));
    let ok = |d: serde_json::Value| (StatusCode::OK, Json(json!({"status": "success", "data": d}))).into_response();
    let bad = |m: &str| (StatusCode::FORBIDDEN, Json(json!({"status": "error", "message": m, "error_type": "InputException"}))).into_response();
    let redirect = |to: &str| (StatusCode::FOUND, [(header::LOCATION, to.to_string())]).into_response();
    match (method.as_str(), uri.path()) {
        ("POST", "/api/login") => {
            if form.get("user_id").map(String::as_str) != Some("AB1234") || form.get("password").map(String::as_str) != Some("pw") {
                return bad("Invalid username or password.");
            }
            let mut r = ok(json!({"request_id": "REQ1", "twofa_type": "totp"}));
            r.headers_mut().insert(header::SET_COOKIE, "kf_session=abc; Path=/".parse().unwrap());
            r
        }
        ("POST", "/api/twofa") => {
            let want = totp::current_code(TOTP_SECRET).unwrap();
            if form.get("request_id").map(String::as_str) != Some("REQ1") || form.get("twofa_type").map(String::as_str) != Some("totp") {
                return bad("bad request");
            }
            // The code can roll over to the next 30 s step between the two calls.
            let prev = totp::code_at(&totp::base32_decode(TOTP_SECRET).unwrap(), kite_engine::accounts::now_secs() - 30, 6);
            let got = form.get("twofa_value").cloned().unwrap_or_default();
            if got != want && got != prev {
                return bad("Invalid TOTP");
            }
            ok(json!({}))
        }
        ("GET", "/connect/login") => {
            // Only a session that finished the two steps (carries the cookie) gets through.
            let cookie = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()).unwrap_or("");
            if !cookie.contains("kf_session=abc") || !uri.query().unwrap_or("").contains("api_key=KEY") {
                return (StatusCode::OK, "login page").into_response();
            }
            redirect("/connect/finish?sess_id=S1")
        }
        ("GET", "/connect/finish") => redirect("http://127.0.0.1:1/app?action=login&type=login&status=success&request_token=RT1"),
        ("POST", "/session/token") => {
            let want = hex::encode(Sha256::digest(b"KEYRT1SECRET"));
            if form.get("checksum") != Some(&want) {
                return bad("bad checksum");
            }
            ok(json!({"access_token": "ACCESS1"}))
        }
        ("GET", "/user/margins") => {
            let auth = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("");
            if auth != "token KEY:ACCESS1" {
                return bad("bad token");
            }
            ok(json!({
                "equity": {"enabled": true, "net": 250000.5,
                    "available": {"live_balance": 250000.5, "collateral": 10000.0, "cash": 240000.5, "opening_balance": 300000.0},
                    "utilised": {"debits": 49999.5, "span": 30000.0, "exposure": 19999.5}},
                "commodity": {"enabled": false, "available": {"live_balance": 999999.0}, "utilised": {"debits": 1.0}}
            }))
        }
        _ => (StatusCode::NOT_FOUND, "no route").into_response(),
    }
}

async fn start() -> (Arc<Accounts>, Calls) {
    let calls: Calls = Arc::default();
    let app = Router::new().fallback(mock).with_state(calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let urls = BrokerUrls::all_at(&base);
    (Accounts::open("master", None, urls, Default::default()).unwrap(), calls)
}

fn zerodha(password: &str) -> Upsert {
    let secrets: BTreeMap<String, String> = [
        ("apiKey", "KEY"),
        ("apiSecret", "SECRET"),
        ("password", password),
        ("totpSecret", TOTP_SECRET),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    Upsert {
        alias: "Self".into(),
        broker: "zerodha".into(),
        user_id: "AB1234".into(),
        auto_login: true,
        fields: BTreeMap::new(),
        secrets,
        settings: json!({"slPct": 50}),
    }
}

#[tokio::test]
async fn signs_in_and_reports_funds() {
    let (acc, calls) = start().await;
    acc.upsert("a1", zerodha("pw")).unwrap();
    assert_eq!(acc.get("a1").unwrap()["session"]["status"], "need");

    let v = acc.login("a1").await.unwrap();
    assert_eq!(v["session"]["status"], "in");
    assert_eq!(v["session"]["available"], 250000.5);
    let f = &v["session"]["funds"];
    assert_eq!((f["available"].as_f64(), f["used"].as_f64(), f["total"].as_f64(), f["collateral"].as_f64()), (Some(250000.5), Some(49999.5), Some(300000.0), Some(10000.0)));
    assert!(f["details"].as_array().unwrap().iter().any(|d| d[0] == "SPAN" && d[1] == 30000.0), "the broker's own breakdown is kept");
    assert!(acc.kite_client("a1").is_some());
    let order: Vec<_> = calls.lock().unwrap().clone();
    assert_eq!(order[..6], ["GET /connect/login", "POST /api/login", "POST /api/twofa", "GET /connect/login", "GET /connect/finish", "POST /session/token"]);
}

#[tokio::test]
async fn the_view_never_contains_a_secret() {
    let (acc, _) = start().await;
    acc.upsert("a1", zerodha("pw")).unwrap();
    acc.login("a1").await.unwrap();
    let all = serde_json::to_string(&acc.list()).unwrap();
    for secret in ["pw\"", "SECRET", TOTP_SECRET, "ACCESS1"] {
        assert!(!all.contains(secret), "{secret} leaked into the account view");
    }
    assert_eq!(acc.get("a1").unwrap()["saved"]["password"], true);
}

#[tokio::test]
async fn wrong_password_fails_clearly_and_stays_signed_out() {
    let (acc, _) = start().await;
    acc.upsert("a1", zerodha("nope")).unwrap();
    let err = acc.login("a1").await.unwrap_err().to_string();
    assert!(err.contains("Invalid username or password"), "{err}");
    let v = acc.get("a1").unwrap();
    assert_eq!(v["session"]["status"], "need");
    assert!(v["session"]["error"].as_str().unwrap().contains("Invalid username"));
}

#[tokio::test]
async fn nothing_signs_in_until_asked() {
    let (acc, calls) = start().await;
    acc.upsert("a1", zerodha("pw")).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(calls.lock().unwrap().is_empty(), "the engine must not sign in on its own");
    assert_eq!(acc.get("a1").unwrap()["session"]["status"], "need");
}

#[tokio::test]
async fn blank_secret_on_update_keeps_the_saved_one() {
    let (acc, _) = start().await;
    acc.upsert("a1", zerodha("pw")).unwrap();
    let mut again = zerodha("");
    again.alias = "Renamed".into();
    again.secrets.retain(|k, _| k == "password");
    let v = acc.upsert("a1", again).unwrap();
    assert_eq!(v["alias"], "Renamed");
    assert_eq!(v["saved"]["apiKey"], true);
    assert_eq!(acc.login("a1").await.unwrap()["session"]["status"], "in");
}

#[tokio::test]
async fn missing_details_are_named() {
    let (acc, _) = start().await;
    let mut u = zerodha("pw");
    u.secrets.remove("totpSecret");
    acc.upsert("a1", u).unwrap();
    let err = acc.login("a1").await.unwrap_err().to_string();
    assert!(err.contains("TOTP secret"), "{err}");
}

#[tokio::test]
async fn another_broker_names_what_is_missing() {
    let (acc, _) = start().await;
    let mut u = zerodha("pw");
    u.broker = "angel".into();
    acc.upsert("a1", u).unwrap();
    // Angel needs a PIN, which a Zerodha account never had.
    let err = acc.login("a1").await.unwrap_err().to_string();
    assert!(err.contains("PIN") && err.contains("not saved"), "{err}");
}

#[tokio::test]
async fn saved_file_is_sealed_and_reopens_with_the_same_key_only() {
    let dir = std::env::temp_dir().join(format!("kite-accounts-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("accounts.enc");
    let open = |key: &str| Accounts::open(key, Some(path.clone()), BrokerUrls::default(), Default::default());

    open("key-one").unwrap().upsert("a1", zerodha("hunter2pw")).unwrap();
    let raw = std::fs::read(&path).unwrap();
    let text = String::from_utf8_lossy(&raw);
    assert!(!text.contains("hunter2pw") && !text.contains(TOTP_SECRET) && !text.contains("AB1234"));

    assert_eq!(open("key-one").unwrap().list().len(), 1);
    assert!(open("key-two").is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn bad_ids_and_brokers_are_refused() {
    let (acc, _) = start().await;
    assert!(acc.upsert("../x", zerodha("pw")).is_err());
    let mut u = zerodha("pw");
    u.broker = "unknown".into();
    assert!(acc.upsert("a1", u).is_err());
    assert!(acc.remove("missing").is_err());
}

#[tokio::test]
async fn margins_can_be_read_again_for_signed_in_accounts_only() {
    let (acc, calls) = start().await;
    acc.upsert("a1", zerodha("pw")).unwrap();
    acc.upsert("a2", zerodha("pw")).unwrap();
    acc.login("a1").await.unwrap();

    // One account that was never signed in cannot be refreshed.
    let err = acc.refresh_funds("a2").await.unwrap_err().to_string();
    assert!(err.contains("sign this account in first"), "{err}");

    let before = calls.lock().unwrap().iter().filter(|c| c.as_str() == "GET /user/margins").count();
    let all = acc.refresh_funds_all().await;
    assert_eq!(all.len(), 1, "only the signed-in account is read");
    assert_eq!(all[0]["id"], "a1");
    assert_eq!(all[0]["session"]["funds"]["used"], 49999.5);
    let after = calls.lock().unwrap().iter().filter(|c| c.as_str() == "GET /user/margins").count();
    assert_eq!(after, before + 1);
}

#[tokio::test]
async fn saved_values_come_back_only_when_asked_for() {
    let (acc, _) = start().await;
    acc.upsert("a1", zerodha("pw")).unwrap();
    // The usual views still carry no secret.
    let all = serde_json::to_string(&acc.list()).unwrap();
    assert!(!all.contains("SECRET") && !all.contains(TOTP_SECRET));
    // Asking for them by name returns exactly what was saved.
    let s = acc.secrets("a1").unwrap();
    assert_eq!(s.get("apiKey").map(String::as_str), Some("KEY"));
    assert_eq!(s.get("apiSecret").map(String::as_str), Some("SECRET"));
    assert_eq!(s.get("totpSecret").map(String::as_str), Some(TOTP_SECRET));
    assert!(acc.secrets("missing").is_err());
}
