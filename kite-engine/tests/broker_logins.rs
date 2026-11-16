//! Automatic sign-in for Angel One, Kotak Neo, Nubra and Upstox, against a local
//! mock of each broker's sign-in. No real broker is contacted. The requests the
//! mock checks (paths, headers, TOTP, base64 PIN) are the ones the proven
//! adapters in D:\Alphahedgedesk send.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use kite_engine::accounts::{now_secs, Accounts, Upsert};
use kite_engine::login::BrokerUrls;
use kite_engine::totp;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

const TOTP: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

type Calls = Arc<Mutex<Vec<String>>>;

/// A code is right if it is this window's, or a neighbour's (the mock and the
/// test can straddle a 30 s boundary).
fn code_ok(code: &str) -> bool {
    let key = totp::base32_decode(TOTP).unwrap();
    let now = now_secs();
    [now - 30, now, now + 30].iter().any(|t| totp::code_at(&key, *t, 6) == code)
}

fn hdr(h: &HeaderMap, n: &str) -> String {
    h.get(n).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
}

async fn mock(State(calls): State<Calls>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    calls.lock().unwrap().push(format!("{method} {}", uri.path()));
    let j: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let form: HashMap<String, String> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let query: HashMap<String, String> = serde_urlencoded::from_str(uri.query().unwrap_or("")).unwrap_or_default();
    let ok = |v: Value| (StatusCode::OK, Json(v)).into_response();
    let redirect = |to: &str| (StatusCode::FOUND, [(header::LOCATION, to.to_string())]).into_response();
    let upstox_fail = |m: &str| (StatusCode::UNAUTHORIZED, Json(json!({"success": false, "error": {"message": m, "errorCode": "UDAPI1"}}))).into_response();

    match (method.as_str(), uri.path()) {
        // ---- Angel One
        ("POST", "/rest/auth/angelbroking/user/v1/loginByPassword") => {
            if hdr(&headers, "x-privatekey") != "AKEY" || hdr(&headers, "x-usertype") != "USER" {
                return ok(json!({"status": false, "message": "Invalid API Key", "errorcode": "AG8004"}));
            }
            if j["clientcode"] != "A123" || j["password"] != "4321" || !code_ok(j["totp"].as_str().unwrap_or("")) {
                // SmartAPI answers a refusal with HTTP 200 and status:false.
                return ok(json!({"status": false, "message": "Invalid totp", "errorcode": "AB1050"}));
            }
            ok(json!({"status": true, "data": {"jwtToken": "JWT1", "refreshToken": "RT1", "feedToken": "FT1"}}))
        }
        ("GET", "/rest/secure/angelbroking/user/v1/getRMS") => {
            if hdr(&headers, "authorization") != "Bearer JWT1" {
                return ok(json!({"status": false, "message": "bad token", "errorcode": "AG8001"}));
            }
            ok(json!({"status": true, "message": "SUCCESS", "errorcode": "", "data": {
                "net": "90000.50", "availablecash": "98765.25", "availableintradaypayin": "0.00", "availablelimitmargin": "0.00",
                "collateral": "0.00", "m2munrealized": "0.00", "m2mrealized": "-120.50", "utiliseddebits": "8764.75",
                "utilisedspan": "5000.00", "utilisedoptionpremium": "0.00", "utilisedholdingsales": "0.00",
                "utilisedexposure": "3764.75", "utilisedturnover": "0.00", "utilisedpayout": "0.00"
            }}))
        }

        // ---- Kotak Neo
        ("POST", "/login/1.0/tradeApiLogin") => {
            if hdr(&headers, "authorization") != "CKEY" || hdr(&headers, "neo-fin-key") != "neotradeapi" {
                return ok(json!({"status": "error", "message": "Missing required field 'NeoFinKey'"}));
            }
            if j["ucc"] == "NOTREG" {
                return ok(json!({"status": "error", "message": "TOTP not registered for this user."}));
            }
            if j["ucc"] != "K1" || j["mobileNumber"] != "+919800000010" || !code_ok(j["totp"].as_str().unwrap_or("")) {
                return ok(json!({"status": "error", "message": "Invalid TOTP"}));
            }
            ok(json!({"data": {"token": "VIEW", "sid": "SID1"}}))
        }
        ("POST", "/login/1.0/tradeApiValidate") => {
            if hdr(&headers, "auth") != "VIEW" || hdr(&headers, "sid") != "SID1" {
                return ok(json!({"status": "error", "message": "bad view token"}));
            }
            // MPIN 777777 gets a session whose first margin read fails; 888888 one whose reads always fail.
            let sid = match j["mpin"].as_str() {
                Some("654321") => "SID2",
                Some("777777") => "SIDF",
                Some("888888") => "SIDDOWN",
                _ => return ok(json!({"status": "error", "message": "Invalid MPIN"})),
            };
            ok(json!({"data": {"token": "TRADE", "sid": sid, "baseUrl": format!("http://{}/", hdr(&headers, "host")), "greetingName": "Wife"}}))
        }

        ("POST", "/quick/user/limits") => {
            if !["SID2", "SIDF", "SIDDOWN"].contains(&hdr(&headers, "sid").as_str()) || hdr(&headers, "auth") != "TRADE" {
                return ok(json!({"stat": "Not_Ok", "emsg": "Invalid session", "stCode": 1003}));
            }
            // An account whose first margin read hits Kotak's temporary 424.
            if hdr(&headers, "sid") == "SIDF" && calls.lock().unwrap().iter().filter(|c| c.as_str() == "POST /quick/user/limits").count() == 1 {
                return (StatusCode::FAILED_DEPENDENCY, Json(json!({"stat": "Not_Ok", "emsg": "bridge API error out", "stCode": 424}))).into_response();
            }
            if hdr(&headers, "sid") == "SIDDOWN" {
                return (StatusCode::FAILED_DEPENDENCY, Json(json!({"stat": "Not_Ok", "emsg": "bridge API error out", "stCode": 424}))).into_response();
            }
            // The body is a form field jData holding JSON.
            let jdata: Value = serde_json::from_str(form.get("jData").map(String::as_str).unwrap_or("null")).unwrap_or(Value::Null);
            if jdata["seg"] != "ALL" || jdata["exch"] != "ALL" || jdata["prod"] != "ALL" {
                return ok(json!({"stat": "Not_Ok", "emsg": "bad jData", "stCode": 1004}));
            }
            ok(json!({
                "Category": "CLIENT_MTF", "EntityId": "account-X", "BoardLotLimit": "5000", "CollateralValue": "10197.48",
                "Net": "10157.08", "MarginUsed": "40.4", "AdhocMargin": "0", "SpanMarginPrsnt": "30.4", "ExposureMarginPrsnt": "10",
                "NotionalCash": "0", "UnrealizedMtomPrsnt": "0", "RealizedMtomPrsnt": "0", "SpecialMarginPrsnt": "0",
                "PremiumPrsnt": "0", "MarginVarPrsnt": "0", "stCode": 200, "stat": "Ok"
            }))
        }

        // ---- Nubra
        ("HEAD", "/") => (StatusCode::OK, [(header::DATE, httpdate::fmt_http_date(std::time::SystemTime::now()))]).into_response(),
        ("POST", "/totp/login") => {
            // The code must arrive as a number.
            let n = j["totp"].as_u64();
            let ok_code = n.is_some_and(|n| code_ok(&format!("{n:06}")));
            if j["phone"] != "9800000010" || !ok_code || hdr(&headers, "x-device-id").is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(json!({"message": "Invalid TOTP"}))).into_response();
            }
            ok(json!({"auth_token": "AUTH1"}))
        }
        ("POST", "/verifypin") => {
            if hdr(&headers, "authorization") != "Bearer AUTH1" || j["pin"] != "2468" {
                return (StatusCode::UNAUTHORIZED, Json(json!({"message": "Invalid PIN"}))).into_response();
            }
            ok(json!({"session_token": "SESS1"}))
        }
        ("GET", "/users/account_info") => ok(json!({"data": {"client_code": "N77", "name": "Papa"}})),

        // ---- Upstox
        ("GET", "/v2/login/authorization/dialog") => {
            if query.get("client_id").map(String::as_str) != Some("UKEY") {
                return (StatusCode::BAD_REQUEST, "bad key").into_response();
            }
            redirect("/signin?user_id=U1")
        }
        ("POST", "/login/open/v6/auth/1fa/otp/generate") => {
            if j["data"]["mobileNumber"] != "9800000010" || j["data"]["userId"] != "U1" {
                return upstox_fail("unknown mobile");
            }
            ok(json!({"success": true, "data": {"validateOTPToken": "VOT", "isTotpEnabled": true}}))
        }
        ("POST", "/login/open/v4/auth/1fa/otp-totp/verify") => {
            if j["data"]["validateOtpToken"] != "VOT" || !code_ok(j["data"]["otp"].as_str().unwrap_or("")) {
                return upstox_fail("Invalid TOTP");
            }
            ok(json!({"success": true, "data": {}}))
        }
        ("POST", "/login/open/v3/auth/2fa") => {
            if j["data"]["inputText"] != "MTIzNA==" {
                return upstox_fail("Invalid PIN");
            }
            // A first-time app has no code yet; flip with the test's marker in the redirect url.
            let approved = query.get("redirect_uri").is_some_and(|r| r.contains("approved"));
            if approved {
                ok(json!({"success": true, "data": {"redirectUri": "http://127.0.0.1:1/approved?code=UC1"}}))
            } else {
                ok(json!({"success": true, "data": {"redirectUri": ""}}))
            }
        }
        ("POST", "/login/v2/oauth/authorize") => {
            if j["data"]["userOAuthApproval"] != true || query.get("requestId").is_none_or(|r| !r.starts_with("WPRO-")) {
                return upstox_fail("not approved");
            }
            ok(json!({"success": true, "data": {"redirectUri": "http://127.0.0.1:1/first?code=UC1"}}))
        }
        ("POST", "/v2/login/authorization/token") => {
            if form.get("code").map(String::as_str) != Some("UC1")
                || form.get("client_secret").map(String::as_str) != Some("USECRETUSECRETUSECRET")
                || form.get("grant_type").map(String::as_str) != Some("authorization_code")
            {
                return (StatusCode::UNAUTHORIZED, Json(json!({"errors": [{"errorCode": "UDAPI100069", "message": "Invalid credentials"}]}))).into_response();
            }
            ok(json!({"access_token": "UP1", "user_id": "UPID", "user_name": "Self"}))
        }
        ("GET", "/v2/user/get-funds-and-margin") => {
            if hdr(&headers, "authorization") != "Bearer UP1" {
                return upstox_fail("bad token");
            }
            ok(json!({"status": "success", "data": {"equity": {"available_margin": 54321.5, "used_margin": 1678.5, "payin_amount": 0, "span_margin": 1000.0, "exposure_margin": 678.5, "adhoc_margin": 0, "notional_cash": 0}}}))
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
    (Accounts::open("master", None, BrokerUrls::all_at(&base), Default::default()).unwrap(), calls)
}

fn put(broker: &str, user_id: &str, secrets: &[(&str, &str)], fields: &[(&str, &str)]) -> Upsert {
    let map = |v: &[(&str, &str)]| v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>();
    Upsert {
        alias: broker.into(),
        broker: broker.into(),
        user_id: user_id.into(),
        auto_login: true,
        fields: map(fields),
        secrets: map(secrets),
        settings: Value::Null,
    }
}

fn angel(pin: &str) -> Upsert {
    put("angel", "A123", &[("apiKey", "AKEY"), ("pin", pin), ("totpSecret", TOTP)], &[])
}
fn kotak(mpin: &str) -> Upsert {
    put("kotak", "K1", &[("apiKey", "CKEY"), ("pin", mpin), ("totpSecret", TOTP)], &[("mobile", "+919800000010")])
}
fn nubra(pin: &str) -> Upsert {
    put("nubra", "", &[("pin", pin), ("totpSecret", TOTP)], &[("mobile", "9800000010")])
}
fn upstox(pin: &str, redirect: &str) -> Upsert {
    put(
        "upstox", "",
        &[("apiKey", "UKEY"), ("apiSecret", "USECRETUSECRETUSECRET"), ("pin", pin), ("totpSecret", TOTP)],
        &[("redirectUrl", redirect), ("mobile", "9800000010")],
    )
}

#[tokio::test]
async fn angel_signs_in_and_reads_funds() {
    let (acc, _) = start().await;
    acc.upsert("a", angel("4321")).unwrap();
    let v = acc.login("a").await.unwrap();
    assert_eq!(v["session"]["status"], "in");
    assert_eq!(v["session"]["available"], 90000.5);
    let f = &v["session"]["funds"];
    assert_eq!((f["used"].as_f64(), f["total"].as_f64()), (Some(8764.75), Some(98765.25)));
    assert!(f["details"].as_array().unwrap().iter().any(|d| d[0] == "Realised P&L" && d[1] == -120.5));
}

#[tokio::test]
async fn angel_failure_is_read_from_the_body_not_the_http_status() {
    let (acc, _) = start().await;
    acc.upsert("a", angel("0000")).unwrap();
    let err = acc.login("a").await.unwrap_err().to_string();
    assert!(err.contains("Invalid totp") && err.contains("AB1050"), "{err}");
    assert_eq!(acc.get("a").unwrap()["session"]["status"], "need");
}

#[tokio::test]
async fn kotak_takes_two_steps_and_keeps_the_trade_token() {
    let (acc, calls) = start().await;
    acc.upsert("k", kotak("654321")).unwrap();
    let v = acc.login("k").await.unwrap();
    assert_eq!(v["session"]["status"], "in");
    assert_eq!(calls.lock().unwrap()[..2], ["POST /login/1.0/tradeApiLogin", "POST /login/1.0/tradeApiValidate"]);
    // Margin comes from the host sign-in returned, as Kotak's docs say.
    let f = &v["session"]["funds"];
    assert_eq!((f["available"].as_f64(), f["used"].as_f64(), f["collateral"].as_f64()), (Some(10157.08), Some(40.4), Some(10197.48)));
    assert!(calls.lock().unwrap().contains(&"POST /quick/user/limits".to_string()));
}

#[tokio::test]
async fn kotak_wrong_mpin_is_reported() {
    let (acc, _) = start().await;
    acc.upsert("k", kotak("000000")).unwrap();
    let err = acc.login("k").await.unwrap_err().to_string();
    assert!(err.contains("Invalid MPIN"), "{err}");
}

#[tokio::test]
async fn nubra_signs_in_with_a_numeric_code_and_a_device_id() {
    let (acc, calls) = start().await;
    acc.upsert("n", nubra("2468")).unwrap();
    let v = acc.login("n").await.unwrap();
    assert_eq!(v["session"]["status"], "in");
    let c = calls.lock().unwrap().clone();
    assert!(c.contains(&"POST /totp/login".to_string()) && c.contains(&"POST /verifypin".to_string()), "{c:?}");
}

#[tokio::test]
async fn nubra_bad_totp_adds_advice() {
    let (acc, _) = start().await;
    let mut u = nubra("2468");
    u.secrets.insert("totpSecret".into(), "MFRGGZDFMZTWQ2LKNNWG23TPOBYXE43UOV3HO6DZPI".into());
    acc.upsert("n", u).unwrap();
    let err = acc.login("n").await.unwrap_err().to_string();
    assert!(err.contains("Invalid TOTP") && err.contains("clock"), "{err}");
}

#[tokio::test]
async fn upstox_signs_in_when_the_app_is_already_approved() {
    let (acc, calls) = start().await;
    acc.upsert("u", upstox("1234", "http://127.0.0.1:1/approved")).unwrap();
    let v = acc.login("u").await.unwrap();
    assert_eq!(v["session"]["status"], "in");
    assert_eq!(v["session"]["available"], 54321.5);
    assert_eq!(v["session"]["funds"]["used"], 1678.5);
    assert!(!calls.lock().unwrap().contains(&"POST /login/v2/oauth/authorize".to_string()), "no approval post needed");
}

#[tokio::test]
async fn upstox_posts_the_approval_the_first_time() {
    let (acc, calls) = start().await;
    acc.upsert("u", upstox("1234", "http://127.0.0.1:1/first")).unwrap();
    assert_eq!(acc.login("u").await.unwrap()["session"]["status"], "in");
    assert!(calls.lock().unwrap().contains(&"POST /login/v2/oauth/authorize".to_string()));
}

#[tokio::test]
async fn upstox_failure_names_the_step_and_offers_the_browser() {
    let (acc, _) = start().await;
    acc.upsert("u", upstox("9999", "http://127.0.0.1:1/approved")).unwrap();
    let err = acc.login("u").await.unwrap_err().to_string();
    assert!(err.contains("PIN") && err.contains("Invalid PIN") && err.contains("sign in once in a browser"), "{err}");
}

#[tokio::test]
async fn upstox_short_secret_gets_a_hint() {
    let (acc, _) = start().await;
    let mut u = upstox("1234", "http://127.0.0.1:1/approved");
    u.secrets.insert("apiSecret".into(), "short".into());
    acc.upsert("u", u).unwrap();
    let err = acc.login("u").await.unwrap_err().to_string();
    assert!(err.contains("suspiciously short") && err.contains("redirect URL matches"), "{err}");
}

#[tokio::test]
async fn missing_details_are_all_named_before_any_request() {
    let (acc, calls) = start().await;
    acc.upsert("u", put("upstox", "", &[("apiKey", "UKEY")], &[])).unwrap();
    let err = acc.login("u").await.unwrap_err().to_string();
    for want in ["API secret", "Redirect URL", "Mobile", "PIN", "TOTP secret"] {
        assert!(err.contains(want), "{want} missing from: {err}");
    }
    assert!(calls.lock().unwrap().is_empty(), "nothing is sent while details are missing");
}

#[tokio::test]
async fn views_never_contain_a_token_or_secret() {
    let (acc, _) = start().await;
    acc.upsert("a", angel("4321")).unwrap();
    acc.upsert("k", kotak("654321")).unwrap();
    acc.login("a").await.unwrap();
    acc.login("k").await.unwrap();
    let all = serde_json::to_string(&acc.list()).unwrap();
    for secret in ["JWT1", "FT1", "TRADE", "SID2", "4321\"", "654321", TOTP, "AKEY", "CKEY"] {
        assert!(!all.contains(secret), "{secret} leaked into the account view");
    }
}

#[tokio::test]
async fn login_all_signs_in_every_marked_account_and_reports_each_one() {
    let (acc, calls) = start().await;
    acc.upsert("a", angel("4321")).unwrap();
    acc.upsert("k", kotak("000000")).unwrap(); // wrong MPIN
    acc.upsert("n", nubra("2468")).unwrap();
    let mut off = upstox("1234", "http://127.0.0.1:1/approved");
    off.auto_login = false; // not marked: must be left alone
    acc.upsert("u", off).unwrap();

    let results = acc.login_all().await;
    assert_eq!(results.len(), 3);
    let by = |id: &str| results.iter().find(|r| r["id"] == id).unwrap().clone();
    assert_eq!(by("a")["ok"], true);
    assert_eq!(by("a")["account"]["session"]["available"], 90000.5);
    assert_eq!(by("n")["ok"], true);
    assert_eq!(by("k")["ok"], false);
    assert!(by("k")["error"].as_str().unwrap().contains("Invalid MPIN"));
    assert_eq!(by("k")["account"]["session"]["status"], "need");
    assert_eq!(acc.get("u").unwrap()["session"]["status"], "need");
    assert!(!calls.lock().unwrap().iter().any(|c| c.contains("/v2/login/authorization")), "the unmarked Upstox account was not touched");
}

#[tokio::test]
async fn kotak_not_registered_explains_where_to_look() {
    let (acc, _) = start().await;
    let mut u = kotak("654321");
    u.user_id = "NOTREG".into();
    acc.upsert("k", u).unwrap();
    let err = acc.login("k").await.unwrap_err().to_string();
    assert!(err.contains("TOTP not registered for this user"), "{err}");
    assert!(err.contains("own Neo API page") || err.contains("Neo API page"), "{err}");
    assert!(err.contains("first 6-digit code"), "{err}");
}

#[tokio::test]
async fn kotak_margin_retries_once_after_a_424() {
    let (acc, calls) = start().await;
    acc.upsert("k", kotak("777777")).unwrap();
    let v = acc.login("k").await.unwrap();
    assert_eq!(v["session"]["funds"]["available"], 10157.08, "the retry got the figures");
    assert_eq!(calls.lock().unwrap().iter().filter(|c| c.as_str() == "POST /quick/user/limits").count(), 2);
}

#[tokio::test]
async fn kotak_margin_down_says_whose_problem_it_is() {
    let (acc, _) = start().await;
    acc.upsert("k", kotak("888888")).unwrap();
    acc.login("k").await.unwrap();
    let v = acc.refresh_funds("k").await.unwrap();
    let err = v["session"]["funds_error"].as_str().unwrap();
    assert!(err.contains("not answering right now") && err.contains("424"), "{err}");
}
