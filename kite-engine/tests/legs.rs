//! The entry + SL engine against the paper broker: the SL is placed from the
//! real fill price, follows partial fills, survives duplicates and failures.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::SinkExt;
use http_body_util::BodyExt;
use kite_engine::api::{router, AppState};
use kite_engine::leg::{LegState, OnSlFailure, SlKind};
use kite_engine::paper::Failure;
use kite_engine::types::*;
use kite_engine::ws::{run_order_stream, WsConfig};
use kite_engine::{Engine, LegManager, LegRequest, ManagerConfig, PaperBroker, RiskLimits, SlSpec};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

const SYM: &str = "NIFTY2611224500CE";

struct Rig {
    paper: Arc<PaperBroker>,
    engine: Arc<Engine>,
    mgr: Arc<LegManager>,
}

fn rig() -> Rig {
    let paper = Arc::new(PaperBroker::new());
    let engine = Arc::new(Engine::new(paper.clone(), RiskLimits::default()));
    let ms = Duration::from_millis;
    let mgr = LegManager::new(
        engine.clone(),
        ManagerConfig { retry_delays: vec![ms(1), ms(1), ms(1)], check_delays: vec![] },
    );
    Rig { paper, engine, mgr }
}

fn entry(side: Side, order_type: OrderType, qty: u32) -> OrderRequest {
    OrderRequest {
        exchange: "NFO".into(),
        tradingsymbol: SYM.into(),
        side,
        order_type,
        product: Product::Nrml,
        quantity: qty,
        price: if order_type == OrderType::Limit { Some(100.0) } else { None },
        trigger_price: None,
        validity: None,
        variety: Variety::Regular,
        tag: None,
        market_protection: None,
    }
}

fn sl(percent: f64) -> SlSpec {
    SlSpec { percent, kind: SlKind::Sl, limit_buffer_pct: 5.0, tick: 0.05, on_failure: OnSlFailure::ExitAtMarket }
}

fn req(id: &str, side: Side, ot: OrderType, qty: u32, s: SlSpec) -> LegRequest {
    LegRequest { leg_id: Some(id.into()), entry: entry(side, ot, qty), sl: s }
}

async fn sl_orders(r: &Rig) -> Vec<Order> {
    r.engine.orders().await.unwrap().into_iter().filter(|o| o.tag.as_deref().map_or(false, |t| t.ends_with('S'))).collect()
}

#[tokio::test]
async fn short_entry_gets_a_buy_sl_at_fifty_percent_of_the_fill() {
    let r = rig();
    r.paper.set_ltp(SYM, 100.0);
    let (id, _) = r.mgr.create_leg(req("T1", Side::Sell, OrderType::Market, 75, sl(50.0))).await.unwrap();
    assert_eq!(r.mgr.get(&id).await.unwrap().state, LegState::PendingEntry);

    r.mgr.reconcile_all().await; // the fill is seen
    let leg = r.mgr.get(&id).await.unwrap();
    assert_eq!(leg.state, LegState::Protected);
    assert_eq!((leg.filled_qty, leg.avg_price), (75, 100.0));
    assert!(leg.fill_seen_ms.is_some() && leg.sl_acked_ms.is_some());

    let s = sl_orders(&r).await;
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].transaction_type, "BUY");
    assert_eq!(s[0].order_type, "SL");
    assert_eq!(s[0].status, "TRIGGER PENDING");
    assert_eq!(s[0].quantity, 75);
    assert_eq!(s[0].trigger_price, 150.0);
    assert_eq!(s[0].price, 157.5);
    assert_eq!(s[0].product, "NRML");
}

#[tokio::test]
async fn long_entry_gets_a_sell_sl_below_the_fill() {
    let r = rig();
    r.paper.set_ltp(SYM, 80.0);
    let (id, _) = r.mgr.create_leg(req("T2", Side::Buy, OrderType::Market, 75, sl(60.0))).await.unwrap();
    r.mgr.reconcile_all().await;
    assert_eq!(r.mgr.get(&id).await.unwrap().state, LegState::Protected);
    let s = &sl_orders(&r).await[0];
    assert_eq!(s.transaction_type, "SELL");
    assert_eq!(s.trigger_price, 32.0); // 80 - 60%
    assert_eq!(s.price, 30.4); // trigger - 5%
}

#[tokio::test]
async fn sl_uses_the_actual_fill_not_the_limit_price() {
    let r = rig();
    let (id, oid) = r.mgr.create_leg(req("T3", Side::Sell, OrderType::Limit, 75, sl(50.0))).await.unwrap();
    // No SL while nothing is filled.
    r.mgr.reconcile_all().await;
    assert!(sl_orders(&r).await.is_empty());
    // Filled at 100.03, better than the 100 limit for a sell.
    r.paper.simulate_fill(&oid, 100.03).unwrap();
    r.mgr.reconcile_all().await;
    let s = &sl_orders(&r).await[0];
    assert_eq!(s.trigger_price, 150.05); // 150.045 rounded up to the tick
    assert_eq!(r.mgr.get(&id).await.unwrap().avg_price, 100.03);
}

#[tokio::test]
async fn partial_fills_grow_the_sl_but_never_past_what_is_filled() {
    let r = rig();
    let (id, oid) = r.mgr.create_leg(req("T4", Side::Sell, OrderType::Limit, 75, sl(50.0))).await.unwrap();

    r.paper.simulate_partial(&oid, 25, 100.0).unwrap();
    r.mgr.reconcile_all().await;
    let s = sl_orders(&r).await;
    assert_eq!((s.len(), s[0].quantity, s[0].trigger_price), (1, 25, 150.0));
    assert_eq!(r.mgr.get(&id).await.unwrap().state, LegState::Protected);

    r.paper.simulate_partial(&oid, 50, 101.0).unwrap();
    r.mgr.reconcile_all().await;
    let s = sl_orders(&r).await;
    assert_eq!((s.len(), s[0].quantity, s[0].trigger_price), (1, 50, 151.5));

    r.paper.simulate_fill(&oid, 102.0).unwrap();
    r.mgr.reconcile_all().await;
    let s = sl_orders(&r).await;
    assert_eq!((s.len(), s[0].quantity, s[0].trigger_price), (1, 75, 153.0));
    let leg = r.mgr.get(&id).await.unwrap();
    assert!(leg.entry_complete);
    assert_eq!(leg.sl_qty, 75);
}

#[tokio::test]
async fn repeated_and_concurrent_updates_never_make_a_second_sl() {
    let r = rig();
    r.paper.set_ltp(SYM, 100.0);
    let (_, oid) = r.mgr.create_leg(req("T5", Side::Sell, OrderType::Market, 75, sl(50.0))).await.unwrap();
    let row = r.engine.order_history(&oid).await.unwrap().pop().unwrap();
    let mut tasks = vec![];
    for _ in 0..20 {
        let (m, row) = (r.mgr.clone(), row.clone());
        tasks.push(tokio::spawn(async move { m.on_update(&row).await }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    r.mgr.reconcile_all().await;
    r.mgr.reconcile_all().await;
    assert_eq!(sl_orders(&r).await.len(), 1);
}

#[tokio::test]
async fn a_fill_that_beats_the_reply_is_found_by_tag() {
    let r = rig();
    r.paper.set_ltp(SYM, 100.0);
    // The update arrives for an order id the leg has not recorded yet; only
    // its tag identifies it.
    let (id, _) = r.mgr.create_leg(req("T6", Side::Sell, OrderType::Limit, 75, sl(50.0))).await.unwrap();
    let early = Order {
        order_id: "UNSEEN-1".into(),
        status: "COMPLETE".into(),
        filled_quantity: 75,
        quantity: 75,
        average_price: 100.0,
        tag: Some("T6".into()),
        ..Default::default()
    };
    r.mgr.on_update(&early).await;
    assert_eq!(r.mgr.get(&id).await.unwrap().state, LegState::Protected);
}

#[tokio::test]
async fn rejected_sl_exits_the_leg_at_market_and_alerts() {
    let r = rig();
    r.paper.set_ltp(SYM, 100.0);
    r.paper.inject("T7S", Failure::Reject("Insufficient funds".into()));
    let mut alerts = r.mgr.subscribe_alerts();
    let (id, _) = r.mgr.create_leg(req("T7", Side::Sell, OrderType::Market, 75, sl(50.0))).await.unwrap();
    r.mgr.reconcile_all().await;

    let leg = r.mgr.get(&id).await.unwrap();
    assert!(matches!(leg.state, LegState::Unprotected(_)), "{:?}", leg.state);
    let a = alerts.try_recv().expect("an alert was sent");
    assert!(a.message.contains("Insufficient funds"));
    // The emergency exit: a market BUY for the filled quantity.
    let exit = r.engine.orders().await.unwrap().into_iter().find(|o| o.tag.as_deref() == Some("T7X")).unwrap();
    assert_eq!((exit.transaction_type.as_str(), exit.order_type.as_str(), exit.quantity), ("BUY", "MARKET", 75));
    // When the exit fills, the leg closes.
    r.mgr.reconcile_all().await;
    assert_eq!(r.mgr.get(&id).await.unwrap().state, LegState::Closed("exited".into()));
}

#[tokio::test]
async fn alert_only_policy_does_not_exit() {
    let r = rig();
    r.paper.set_ltp(SYM, 100.0);
    r.paper.inject("T8S", Failure::Reject("RMS block".into()));
    let mut s = sl(50.0);
    s.on_failure = OnSlFailure::AlertOnly;
    let (id, _) = r.mgr.create_leg(req("T8", Side::Sell, OrderType::Market, 75, s)).await.unwrap();
    r.mgr.reconcile_all().await;
    assert!(matches!(r.mgr.get(&id).await.unwrap().state, LegState::Unprotected(_)));
    assert!(r.engine.orders().await.unwrap().iter().all(|o| o.tag.as_deref() != Some("T8X")));
}

#[tokio::test]
async fn network_failures_are_retried_and_end_with_one_sl() {
    let r = rig();
    r.paper.set_ltp(SYM, 100.0);
    r.paper.inject("T9S", Failure::Network(2));
    let (id, _) = r.mgr.create_leg(req("T9", Side::Sell, OrderType::Market, 75, sl(50.0))).await.unwrap();
    r.mgr.reconcile_all().await;
    assert_eq!(r.mgr.get(&id).await.unwrap().state, LegState::Protected);
    assert_eq!(sl_orders(&r).await.len(), 1);
}

#[tokio::test]
async fn a_lost_reply_does_not_create_a_second_sl() {
    let r = rig();
    r.paper.set_ltp(SYM, 100.0);
    // The SL reaches the broker but the reply times out.
    r.paper.inject("T10S", Failure::UnknownAfterPlace(1));
    let (id, _) = r.mgr.create_leg(req("T10", Side::Sell, OrderType::Market, 75, sl(50.0))).await.unwrap();
    r.mgr.reconcile_all().await;
    let leg = r.mgr.get(&id).await.unwrap();
    assert_eq!(leg.state, LegState::Protected);
    assert_eq!(sl_orders(&r).await.len(), 1, "the SL found by its tag is adopted, not repeated");
    assert!(leg.sl_order_id.is_some());
}

#[tokio::test]
async fn a_lost_entry_reply_is_adopted_by_tag() {
    let r = rig();
    r.paper.set_ltp(SYM, 100.0);
    r.paper.inject("T11", Failure::UnknownAfterPlace(1));
    let err = r.mgr.create_leg(req("T11", Side::Sell, OrderType::Market, 75, sl(50.0))).await.unwrap_err();
    assert!(matches!(err, kite_engine::Error::Unknown(_)));
    r.mgr.reconcile_all().await;
    let leg = r.mgr.get("T11").await.unwrap();
    assert!(leg.entry_order_id.is_some());
    assert_eq!(leg.state, LegState::Protected);
    // Exactly one entry order exists.
    let entries = r.engine.orders().await.unwrap().into_iter().filter(|o| o.tag.as_deref() == Some("T11")).count();
    assert_eq!(entries, 1);
}

#[tokio::test]
async fn sl_fill_closes_the_leg() {
    let r = rig();
    r.paper.set_ltp(SYM, 100.0);
    let (id, _) = r.mgr.create_leg(req("T12", Side::Sell, OrderType::Market, 75, sl(50.0))).await.unwrap();
    r.mgr.reconcile_all().await;
    let sl_id = r.mgr.get(&id).await.unwrap().sl_order_id.unwrap();
    r.paper.simulate_fill(&sl_id, 151.0).unwrap();
    r.mgr.reconcile_all().await;
    let leg = r.mgr.get(&id).await.unwrap();
    assert_eq!(leg.state, LegState::Closed("sl_filled".into()));
    assert_eq!(leg.exit_price, Some(151.0));
}

#[tokio::test]
async fn rejected_entry_closes_the_leg_with_no_sl() {
    let r = rig();
    r.paper.inject("T13", Failure::Reject("no margin".into()));
    let err = r.mgr.create_leg(req("T13", Side::Sell, OrderType::Market, 75, sl(50.0))).await.unwrap_err();
    assert!(matches!(err, kite_engine::Error::Api { .. }));
    assert!(r.mgr.get("T13").await.is_none(), "a refused entry leaves no leg behind");
    assert!(sl_orders(&r).await.is_empty());
}

#[tokio::test]
async fn exit_leg_cancels_the_sl_and_closes_at_market() {
    let r = rig();
    r.paper.set_ltp(SYM, 100.0);
    let (id, _) = r.mgr.create_leg(req("T14", Side::Sell, OrderType::Market, 75, sl(50.0))).await.unwrap();
    r.mgr.reconcile_all().await;
    r.mgr.exit_leg(&id).await.unwrap();
    assert_eq!(sl_orders(&r).await[0].status, "CANCELLED");
    r.mgr.reconcile_all().await;
    assert_eq!(r.mgr.get(&id).await.unwrap().state, LegState::Closed("exited".into()));
    // Not unprotected, and no alert-style state in between.
}

#[tokio::test]
async fn changing_the_sl_percent_moves_the_sl() {
    let r = rig();
    r.paper.set_ltp(SYM, 100.0);
    let (id, _) = r.mgr.create_leg(req("T15", Side::Sell, OrderType::Market, 75, sl(50.0))).await.unwrap();
    r.mgr.reconcile_all().await;
    r.mgr.set_sl_percent(&id, 60.0).await.unwrap();
    let s = sl_orders(&r).await;
    assert_eq!((s.len(), s[0].trigger_price, s[0].price), (1, 160.0, 168.0));
    assert!(r.mgr.set_sl_percent(&id, -1.0).await.is_err());
}

#[tokio::test]
async fn bad_leg_requests_are_refused_before_any_order() {
    let r = rig();
    let mut q = req("T16", Side::Sell, OrderType::Market, 75, sl(50.0));
    q.entry.tag = Some("X".into());
    assert!(r.mgr.create_leg(q).await.is_err());
    assert!(r.mgr.create_leg(req("T17", Side::Buy, OrderType::Market, 75, sl(100.0))).await.is_err());
    assert!(r.mgr.create_leg(req("bad id!", Side::Sell, OrderType::Market, 75, sl(50.0))).await.is_err());
    assert!(r.engine.orders().await.unwrap().is_empty());
}

// ------------------------------------------------------------------ socket

#[tokio::test]
async fn order_stream_message_triggers_the_sl_and_reconnects() {
    let r = rig();
    let (_, oid) = r.mgr.create_leg(req("W1", Side::Sell, OrderType::Limit, 75, sl(50.0))).await.unwrap();

    // A mock of Kite's socket: first connection gets a heartbeat, then the
    // fill, then is dropped; the second connection just stays open.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (conns_tx, mut conns_rx) = tokio::sync::mpsc::unbounded_channel();
    let fill = json!({"type":"order","data":{
        "order_id": oid, "status":"COMPLETE", "filled_quantity":75, "quantity":75,
        "average_price":100.0, "tag":"W1"}})
    .to_string();
    tokio::spawn(async move {
        use tokio_tungstenite::tungstenite::Message;
        let mut n = 0;
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            n += 1;
            conns_tx.send(n).unwrap();
            if n == 1 {
                ws.send(Message::Binary(vec![0u8].into())).await.unwrap(); // heartbeat
                ws.send(Message::Text(fill.clone().into())).await.unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
                drop(ws); // the link drops
            } else {
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        }
    });

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let cfg = WsConfig {
        url: format!("ws://{addr}"),
        idle_timeout: Duration::from_secs(5),
        backoff_min: Duration::from_millis(20),
        backoff_max: Duration::from_millis(100),
    };
    tokio::spawn(run_order_stream(cfg, r.mgr.clone(), stop_rx));

    // The fill message alone (the paper order is still open) places the SL.
    let mut protected = false;
    for _ in 0..100 {
        if r.mgr.get("W1").await.unwrap().state == LegState::Protected {
            protected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(protected, "the SL was not placed from the socket message");
    assert_eq!(sl_orders(&r).await.len(), 1);

    // It reconnected after the drop.
    assert_eq!(conns_rx.recv().await, Some(1));
    let second = tokio::time::timeout(Duration::from_secs(3), conns_rx.recv()).await;
    assert_eq!(second, Ok(Some(2)), "the stream did not reconnect");
    let _ = stop_tx.send(true);
}

#[tokio::test]
async fn a_silent_socket_is_treated_as_dead_and_reopened() {
    let r = rig();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut held = vec![];
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            held.push(ws); // accept and say nothing, ever
            tx.send(held.len()).unwrap();
        }
    });
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let cfg = WsConfig {
        url: format!("ws://{addr}"),
        idle_timeout: Duration::from_millis(150),
        backoff_min: Duration::from_millis(10),
        backoff_max: Duration::from_millis(50),
    };
    tokio::spawn(run_order_stream(cfg, r.mgr.clone(), stop_rx));
    assert_eq!(rx.recv().await, Some(1));
    assert_eq!(tokio::time::timeout(Duration::from_secs(3), rx.recv()).await, Ok(Some(2)));
    let _ = stop_tx.send(true);
}

// ---------------------------------------------------------- HTTP + postback

fn app(secret: Option<&str>) -> (axum::Router, Rig) {
    let r = rig();
    let state = AppState {
        engine: r.engine.clone(),
        legs: r.mgr.clone(),
        token: Arc::new("test-token-0123456789".into()),
        postback_secret: secret.map(|s| Arc::new(s.to_string())),
        paper: Some(r.paper.clone()),
        accounts: kite_engine::accounts::Accounts::open("test-master", None, Default::default(), Default::default()).unwrap(),
    };
    (router(state), r)
}

async fn call(app: &axum::Router, method: &str, path: &str, auth: bool, body: Option<Value>) -> (StatusCode, Value) {
    let mut rb = Request::builder().method(method).uri(path);
    if auth {
        rb = rb.header("authorization", "Bearer test-token-0123456789");
    }
    let req = match body {
        Some(b) => rb.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => rb.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn http_flow_create_leg_fill_by_hand_and_read_it_back() {
    let (a, _r) = app(None);
    let body = json!({
        "leg_id": "H1",
        "entry": {"exchange":"NFO","tradingsymbol":SYM,"side":"SELL","order_type":"LIMIT",
                  "product":"NRML","quantity":75,"price":100.0},
        "sl": {"percent": 50.0}
    });
    assert_eq!(call(&a, "POST", "/legs", false, Some(body.clone())).await.0, StatusCode::UNAUTHORIZED);
    let (s, b) = call(&a, "POST", "/legs", true, Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let oid = b["entry_order_id"].as_str().unwrap().to_string();

    let (_, b) = call(&a, "GET", "/legs/H1", true, None).await;
    assert_eq!(b["state"]["status"], "pending_entry");

    let (s, _) = call(&a, "POST", "/paper/fill", true, Some(json!({"order_id": oid, "price": 100.0}))).await;
    assert_eq!(s, StatusCode::OK);
    let (_, b) = call(&a, "GET", "/legs/H1", true, None).await;
    assert_eq!(b["state"]["status"], "protected");
    assert_eq!(b["sl_trigger"], 150.0);
    assert_eq!(b["sl_price"], 157.5);

    let (_, b) = call(&a, "PUT", "/legs/H1/sl", true, Some(json!({"percent": 40.0}))).await;
    assert_eq!(b["sl_trigger"], 140.0);

    let (s, _) = call(&a, "POST", "/legs/H1/exit", true, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(call(&a, "GET", "/legs/NOPE", true, None).await.0, StatusCode::NOT_FOUND);
    let (_, b) = call(&a, "GET", "/legs", true, None).await;
    assert_eq!(b["legs"].as_array().unwrap().len(), 1);
}

fn postback_body(order_id: &str, ts: &str, secret: &str, tag: &str) -> Value {
    let checksum = hex::encode(Sha256::digest(format!("{order_id}{ts}{secret}").as_bytes()));
    json!({"order_id": order_id, "order_timestamp": ts, "checksum": checksum, "status": "COMPLETE",
           "filled_quantity": 75, "quantity": 75, "average_price": 100.0, "tag": tag})
}

#[tokio::test]
async fn postback_with_a_valid_checksum_places_the_sl() {
    let (a, r) = app(Some("api-secret"));
    let (_, oid) = r.mgr.create_leg(req("P1", Side::Sell, OrderType::Limit, 75, sl(50.0))).await.unwrap();
    let (s, _) = call(&a, "POST", "/postback/zerodha", false, Some(postback_body(&oid, "2026-11-16 10:00:00", "api-secret", "P1"))).await;
    assert_eq!(s, StatusCode::OK);
    for _ in 0..100 {
        if r.mgr.get("P1").await.unwrap().state == LegState::Protected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the postback did not lead to an SL");
}

#[tokio::test]
async fn postback_with_a_wrong_checksum_or_no_secret_is_refused() {
    let (a, r) = app(Some("api-secret"));
    let (_, oid) = r.mgr.create_leg(req("P2", Side::Sell, OrderType::Limit, 75, sl(50.0))).await.unwrap();
    // Signed with a different secret: a forgery.
    let (s, _) = call(&a, "POST", "/postback/zerodha", false, Some(postback_body(&oid, "2026-11-16 10:00:00", "wrong", "P2"))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = call(&a, "POST", "/postback/zerodha", false, Some(json!({"order_id": oid}))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(r.mgr.get("P2").await.unwrap().state, LegState::PendingEntry);
    assert!(sl_orders(&r).await.is_empty());

    let (a2, _) = app(None);
    let (s, _) = call(&a2, "POST", "/postback/zerodha", false, Some(json!({}))).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "postbacks are off without the API secret");
}
