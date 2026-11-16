//! Checks KiteClient against a local mock of Kite's API: what we send (method,
//! path, headers, form) and how we read what comes back. No real broker is used.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::{Json, Router};
use kite_engine::types::*;
use kite_engine::{Broker, Error, KiteClient, KiteOptions};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    path: String,
    auth: String,
    version: String,
    form: HashMap<String, String>,
}

type Log = Arc<Mutex<Vec<Recorded>>>;

async fn mock(State(log): State<Log>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    let form: HashMap<String, String> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    log.lock().unwrap().push(Recorded {
        method: method.to_string(),
        path: uri.path().to_string(),
        auth: h("authorization"),
        version: h("x-kite-version"),
        form: form.clone(),
    });
    let ok = |d: serde_json::Value| (StatusCode::OK, Json(json!({"status": "success", "data": d})));
    match (method.as_str(), uri.path()) {
        ("POST", "/orders/regular") if form.get("tradingsymbol").map(String::as_str) == Some("REJECTME") => (
            StatusCode::BAD_REQUEST,
            Json(json!({"status":"error","message":"Insufficient funds.","error_type":"MarginException"})),
        ),
        ("POST", "/orders/regular") => ok(json!({"order_id": "2611160001"})),
        ("PUT", p) if p.starts_with("/orders/regular/") => ok(json!({"order_id": p.rsplit('/').next().unwrap()})),
        ("DELETE", p) if p.starts_with("/orders/regular/") => ok(json!({"order_id": p.rsplit('/').next().unwrap()})),
        ("GET", "/orders") => ok(json!([
            {"order_id":"1","status":"COMPLETE","status_message":null,"tradingsymbol":"NIFTY2611224500CE",
             "exchange":"NFO","transaction_type":"SELL","order_type":"MARKET","product":"NRML",
             "quantity":75,"filled_quantity":75,"pending_quantity":0,"price":0,"trigger_price":0,
             "average_price":101.35,"tag":null,"unknown_new_field":"ignored"},
            {"order_id":"2","status":"TRIGGER PENDING","tradingsymbol":"NIFTY2611224500CE","quantity":75,
             "trigger_price":152.05,"price":153.0,"tag":"AHSL1"}
        ])),
        ("GET", "/orders/1") => ok(json!([
            {"order_id":"1","status":"PUT ORDER REQ RECEIVED"},{"order_id":"1","status":"COMPLETE","average_price":101.35}
        ])),
        ("GET", "/portfolio/positions") => ok(json!({
            "net":[{"tradingsymbol":"NIFTY2611224500CE","exchange":"NFO","product":"NRML","quantity":-75,
                    "average_price":101.35,"last_price":99.0,"pnl":176.25,"buy_quantity":0,"sell_quantity":75}],
            "day":[]
        })),
        ("GET", "/user/margins") => ok(json!({"equity":{"net":500000.0}})),
        _ => (StatusCode::NOT_FOUND, Json(json!({"status":"error","message":"no route","error_type":"GeneralException"}))),
    }
}

async fn start() -> (KiteClient, Log) {
    let log: Log = Arc::default();
    let app = Router::new().fallback(mock).with_state(log.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let opts = KiteOptions { base_url: Some(format!("http://{addr}")), ..Default::default() };
    (KiteClient::with_options("KEY", "TOKEN", opts).unwrap(), log)
}

fn market_sell() -> OrderRequest {
    OrderRequest {
        exchange: "NFO".into(),
        tradingsymbol: "NIFTY2611224500CE".into(),
        side: Side::Sell,
        order_type: OrderType::Market,
        product: Product::Nrml,
        quantity: 75,
        price: None,
        trigger_price: None,
        validity: None,
        variety: Variety::Regular,
        tag: Some("AH1".into()),
        market_protection: None,
    }
}

#[tokio::test]
async fn place_sends_the_right_request() {
    let (c, log) = start().await;
    let id = c.place_order(&market_sell()).await.unwrap();
    assert_eq!(id, "2611160001");
    let r = log.lock().unwrap()[0].clone();
    assert_eq!(r.method, "POST");
    assert_eq!(r.path, "/orders/regular");
    assert_eq!(r.auth, "token KEY:TOKEN");
    assert_eq!(r.version, "3");
    assert_eq!(r.form["exchange"], "NFO");
    assert_eq!(r.form["tradingsymbol"], "NIFTY2611224500CE");
    assert_eq!(r.form["transaction_type"], "SELL");
    assert_eq!(r.form["order_type"], "MARKET");
    assert_eq!(r.form["product"], "NRML");
    assert_eq!(r.form["quantity"], "75");
    assert_eq!(r.form["tag"], "AH1");
    assert!(!r.form.contains_key("price"), "a MARKET order must not send a price");
    assert!(!r.form.contains_key("trigger_price"));
}

#[tokio::test]
async fn sl_order_sends_trigger_and_price() {
    let (c, log) = start().await;
    let mut r = market_sell();
    r.side = Side::Buy;
    r.order_type = OrderType::Sl;
    r.trigger_price = Some(152.05);
    r.price = Some(153.0);
    c.place_order(&r).await.unwrap();
    let f = log.lock().unwrap()[0].form.clone();
    assert_eq!(f["order_type"], "SL");
    assert_eq!(f["trigger_price"], "152.05");
    assert_eq!(f["price"], "153");
    assert_eq!(f["transaction_type"], "BUY");
}

#[tokio::test]
async fn broker_rejection_is_mapped() {
    let (c, _) = start().await;
    let mut r = market_sell();
    r.tradingsymbol = "REJECTME".into();
    match c.place_order(&r).await {
        Err(Error::Api { kind, message }) => {
            assert_eq!(kind, "MarginException");
            assert_eq!(message, "Insufficient funds.");
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[tokio::test]
async fn invalid_order_never_reaches_the_broker() {
    let (c, log) = start().await;
    let mut r = market_sell();
    r.quantity = 0;
    assert!(matches!(c.place_order(&r).await, Err(Error::Validation(_))));
    assert!(log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn modify_and_cancel() {
    let (c, log) = start().await;
    let m = ModifyRequest { trigger_price: Some(160.0), price: Some(161.0), ..Default::default() };
    assert_eq!(c.modify_order(Variety::Regular, "77", &m).await.unwrap(), "77");
    assert_eq!(c.cancel_order(Variety::Regular, "77").await.unwrap(), "77");
    let l = log.lock().unwrap();
    assert_eq!((l[0].method.as_str(), l[0].path.as_str()), ("PUT", "/orders/regular/77"));
    assert_eq!(l[0].form["trigger_price"], "160");
    assert_eq!(l[0].form["price"], "161");
    assert!(!l[0].form.contains_key("quantity"));
    assert_eq!((l[1].method.as_str(), l[1].path.as_str()), ("DELETE", "/orders/regular/77"));
}

#[tokio::test]
async fn reads_orders_history_positions_margins() {
    let (c, _) = start().await;
    let orders = c.orders().await.unwrap();
    assert_eq!(orders.len(), 2);
    assert_eq!(orders[0].average_price, 101.35);
    assert!(!orders[0].is_open());
    assert!(orders[1].is_open());
    assert_eq!(orders[1].trigger_price, 152.05);
    assert_eq!(orders[1].tag.as_deref(), Some("AHSL1"));
    let h = c.order_history("1").await.unwrap();
    assert_eq!(h.len(), 2);
    assert_eq!(h[1].status, "COMPLETE");
    let p = c.positions().await.unwrap();
    assert_eq!(p.net[0].quantity, -75);
    assert_eq!(p.net[0].pnl, 176.25);
    let m = c.margins().await.unwrap();
    assert_eq!(m["equity"]["net"], 500000.0);
}

#[tokio::test]
async fn connection_refused_is_network_error_not_unknown() {
    // Port 1 is closed: nothing can have been sent, so a retry is safe.
    let opts = KiteOptions { base_url: Some("http://127.0.0.1:1".into()), ..Default::default() };
    let c = KiteClient::with_options("K", "T", opts).unwrap();
    assert!(matches!(c.place_order(&market_sell()).await, Err(Error::Network(_))));
}

#[test]
fn debug_output_hides_keys() {
    let c = KiteClient::new("SECRETKEY", "SECRETTOKEN").unwrap();
    let s = format!("{c:?}");
    assert!(!s.contains("SECRETKEY") && !s.contains("SECRETTOKEN"));
}
