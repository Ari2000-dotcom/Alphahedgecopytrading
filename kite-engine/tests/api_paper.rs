//! The REST API end to end, in paper mode: auth, place, modify, cancel, reads,
//! risk limit, duplicate guard.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kite_engine::api::{router, AppState};
use kite_engine::{Engine, LegManager, ManagerConfig, PaperBroker, RiskLimits};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789";

fn app() -> (axum::Router, Arc<PaperBroker>) {
    let paper = Arc::new(PaperBroker::new());
    let engine = Arc::new(Engine::new(paper.clone(), RiskLimits { max_quantity: 1800 }));
    let legs = LegManager::new(engine.clone(), ManagerConfig { retry_delays: vec![], check_delays: vec![] });
    let state = AppState { engine, legs, token: Arc::new(TOKEN.into()), postback_secret: None, paper: Some(paper.clone()) };
    (router(state), paper)
}

async fn call(app: &axum::Router, method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut rb = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        rb = rb.header("authorization", format!("Bearer {t}"));
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

fn order(extra: Value) -> Value {
    let mut base = json!({
        "exchange":"NFO","tradingsymbol":"NIFTY2611224500CE","side":"SELL",
        "order_type":"MARKET","product":"NRML","quantity":75
    });
    for (k, v) in extra.as_object().unwrap() {
        base[k] = v.clone();
    }
    base
}

#[tokio::test]
async fn health_is_open_but_everything_else_needs_the_token() {
    let (a, _) = app();
    assert_eq!(call(&a, "GET", "/health", None, None).await.0, StatusCode::OK);
    assert_eq!(call(&a, "GET", "/orders", None, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&a, "GET", "/orders", Some("wrong"), None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&a, "POST", "/orders", None, Some(order(json!({})))).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&a, "GET", "/orders", Some(TOKEN), None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn place_market_then_read_back_order_and_position() {
    let (a, paper) = app();
    paper.set_ltp("NIFTY2611224500CE", 101.0);
    let (s, b) = call(&a, "POST", "/orders", Some(TOKEN), Some(order(json!({"tag":"AH1"})))).await;
    assert_eq!(s, StatusCode::OK);
    let id = b["order_id"].as_str().unwrap().to_string();

    let (_, b) = call(&a, "GET", "/orders", Some(TOKEN), None).await;
    assert_eq!(b["orders"][0]["status"], "COMPLETE");
    assert_eq!(b["orders"][0]["average_price"], 101.0);
    assert_eq!(b["orders"][0]["tag"], "AH1");

    let (s, b) = call(&a, "GET", &format!("/orders/{id}"), Some(TOKEN), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["history"][0]["order_id"], id);

    let (_, b) = call(&a, "GET", "/positions", Some(TOKEN), None).await;
    assert_eq!(b["net"][0]["quantity"], -75);
    assert_eq!(b["net"][0]["average_price"], 101.0);
}

#[tokio::test]
async fn modify_and_cancel_a_resting_order() {
    let (a, _) = app();
    let (_, b) = call(&a, "POST", "/orders", Some(TOKEN),
        Some(order(json!({"order_type":"LIMIT","price":120.0,"side":"BUY"})))).await;
    let id = b["order_id"].as_str().unwrap().to_string();

    let (s, _) = call(&a, "PUT", &format!("/orders/regular/{id}"), Some(TOKEN), Some(json!({"price":118.5,"quantity":150}))).await;
    assert_eq!(s, StatusCode::OK);
    let (_, b) = call(&a, "GET", "/orders", Some(TOKEN), None).await;
    assert_eq!(b["orders"][0]["price"], 118.5);
    assert_eq!(b["orders"][0]["quantity"], 150);

    let (s, _) = call(&a, "DELETE", &format!("/orders/regular/{id}"), Some(TOKEN), None).await;
    assert_eq!(s, StatusCode::OK);
    let (_, b) = call(&a, "GET", "/orders", Some(TOKEN), None).await;
    assert_eq!(b["orders"][0]["status"], "CANCELLED");

    // A cancelled order cannot be changed or cancelled again.
    let (s, b) = call(&a, "DELETE", &format!("/orders/regular/{id}"), Some(TOKEN), None).await;
    assert_eq!(s, StatusCode::BAD_GATEWAY);
    assert_eq!(b["error"], "broker_rejected");
}

#[tokio::test]
async fn sl_order_rests_until_filled() {
    let (a, paper) = app();
    let (_, b) = call(&a, "POST", "/orders", Some(TOKEN), Some(order(json!({
        "side":"BUY","order_type":"SL","trigger_price":150.0,"price":152.0,"tag":"AHSL1"})))).await;
    let id = b["order_id"].as_str().unwrap().to_string();
    let (_, b) = call(&a, "GET", "/orders", Some(TOKEN), None).await;
    assert_eq!(b["orders"][0]["status"], "TRIGGER PENDING");
    paper.simulate_fill(&id, 150.5).unwrap();
    let (_, b) = call(&a, "GET", "/orders", Some(TOKEN), None).await;
    assert_eq!(b["orders"][0]["status"], "COMPLETE");
    assert_eq!(b["orders"][0]["average_price"], 150.5);
}

#[tokio::test]
async fn risk_limit_and_validation_and_duplicates() {
    let (a, _) = app();
    let (s, b) = call(&a, "POST", "/orders", Some(TOKEN), Some(order(json!({"quantity":1801})))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(b["error"], "risk");

    let (s, b) = call(&a, "POST", "/orders", Some(TOKEN), Some(order(json!({"order_type":"LIMIT"})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["error"], "validation");

    let (s, _) = call(&a, "POST", "/orders", Some(TOKEN), Some(order(json!({"tag":"ONCE"})))).await;
    assert_eq!(s, StatusCode::OK);
    let (s, b) = call(&a, "POST", "/orders", Some(TOKEN), Some(order(json!({"tag":"ONCE"})))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(b["error"], "duplicate");
    let (_, b) = call(&a, "GET", "/orders", Some(TOKEN), None).await;
    assert_eq!(b["orders"].as_array().unwrap().len(), 1, "the duplicate must not create a second order");
}

#[tokio::test]
async fn unknown_order_is_404() {
    let (a, _) = app();
    let (s, _) = call(&a, "GET", "/orders/NOPE", Some(TOKEN), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}
