//! The REST API the desktop app (or curl) talks to.
//!
//! Every route needs `Authorization: Bearer <ENGINE_TOKEN>` except:
//!  - `/health`
//!  - `/postback/zerodha`, which Zerodha calls and which is checked by the
//!    postback checksum instead (only enabled when the API secret is set).

use crate::accounts::{Accounts, Upsert};
use crate::engine::Engine;
use crate::error::Error;
use crate::leg::{LegManager, LegRequest};
use crate::paper::PaperBroker;
use crate::types::{ModifyRequest, Order, OrderRequest, Variety};
use axum::body::Bytes;
use axum::extract::{Path, Request, State};
use axum::http::{header::AUTHORIZATION, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<Engine>,
    pub legs: Arc<LegManager>,
    pub token: Arc<String>,
    /// The Kite app's API secret, used to check postbacks. None = postbacks off.
    pub postback_secret: Option<Arc<String>>,
    /// Present only in paper mode: lets you fill orders by hand.
    pub paper: Option<Arc<PaperBroker>>,
    pub accounts: Arc<Accounts>,
}

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/orders", post(place).get(list_orders))
        .route("/orders/:id", get(order_history))
        .route("/orders/:variety/:id", put(modify).delete(cancel))
        .route("/positions", get(positions))
        .route("/margins", get(margins))
        .route("/legs", post(create_leg).get(list_legs))
        .route("/legs/:id", get(get_leg))
        .route("/legs/:id/exit", post(exit_leg))
        .route("/legs/:id/sl", put(set_sl))
        .route("/accounts", get(list_accounts))
        .route("/accounts/login-all", post(login_all))
        .route("/accounts/funds", post(refresh_funds))
        .route("/accounts/:id/funds", post(refresh_one_funds))
        .route("/accounts/:id", get(get_account).put(put_account).delete(delete_account))
        .route("/accounts/:id/login", post(login_account))
        .route("/accounts/:id/secrets", post(account_secrets))
        .route("/paper/fill", post(paper_fill))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token));
    Router::new()
        .route("/health", get(health))
        .route("/postback/zerodha", post(postback))
        .merge(protected)
        .with_state(state)
}

/// Compares without stopping at the first different byte.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn require_token(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| constant_time_eq(t.as_bytes(), st.token.as_bytes()))
        .unwrap_or(false);
    if !ok {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "unauthorized"}))).into_response();
    }
    next.run(req).await
}

pub struct ApiError(Error);

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (code, kind) = match &self.0 {
            Error::Validation(_) => (StatusCode::BAD_REQUEST, "validation"),
            Error::Risk(_) => (StatusCode::UNPROCESSABLE_ENTITY, "risk"),
            Error::Duplicate(_) => (StatusCode::CONFLICT, "duplicate"),
            Error::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            Error::Api { .. } => (StatusCode::BAD_GATEWAY, "broker_rejected"),
            Error::Network(_) => (StatusCode::BAD_GATEWAY, "network"),
            Error::Unknown(_) => (StatusCode::GATEWAY_TIMEOUT, "outcome_unknown"),
            Error::BadResponse(_) => (StatusCode::BAD_GATEWAY, "bad_response"),
        };
        (code, Json(json!({"error": kind, "message": self.0.to_string()}))).into_response()
    }
}

type ApiResult = Result<Json<Value>, ApiError>;

async fn health(State(st): State<AppState>) -> Json<Value> {
    Json(json!({"ok": true, "broker": st.engine.broker_name()}))
}

// ------------------------------------------------------------ plain orders

async fn place(State(st): State<AppState>, Json(req): Json<OrderRequest>) -> ApiResult {
    let id = st.engine.place(&req).await?;
    Ok(Json(json!({"order_id": id})))
}

async fn modify(
    State(st): State<AppState>,
    Path((variety, id)): Path<(Variety, String)>,
    Json(req): Json<ModifyRequest>,
) -> ApiResult {
    let id = st.engine.modify(variety, &id, &req).await?;
    Ok(Json(json!({"order_id": id})))
}

async fn cancel(State(st): State<AppState>, Path((variety, id)): Path<(Variety, String)>) -> ApiResult {
    let id = st.engine.cancel(variety, &id).await?;
    Ok(Json(json!({"order_id": id})))
}

async fn list_orders(State(st): State<AppState>) -> ApiResult {
    Ok(Json(json!({"orders": st.engine.orders().await?})))
}

async fn order_history(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(json!({"history": st.engine.order_history(&id).await?})))
}

async fn positions(State(st): State<AppState>) -> ApiResult {
    Ok(Json(serde_json::to_value(st.engine.positions().await?).map_err(|e| Error::BadResponse(e.to_string()))?))
}

async fn margins(State(st): State<AppState>) -> ApiResult {
    Ok(Json(st.engine.margins().await?))
}

// -------------------------------------------------------------------- legs

async fn create_leg(State(st): State<AppState>, Json(req): Json<LegRequest>) -> ApiResult {
    let (leg_id, order_id) = st.legs.create_leg(req).await?;
    Ok(Json(json!({"leg_id": leg_id, "entry_order_id": order_id})))
}

async fn list_legs(State(st): State<AppState>) -> ApiResult {
    Ok(Json(json!({"legs": st.legs.list().await})))
}

async fn get_leg(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let leg = st.legs.get(&id).await.ok_or_else(|| Error::NotFound(id))?;
    Ok(Json(serde_json::to_value(leg).map_err(|e| Error::BadResponse(e.to_string()))?))
}

async fn exit_leg(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    st.legs.exit_leg(&id).await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
struct SetSl {
    percent: f64,
}

async fn set_sl(State(st): State<AppState>, Path(id): Path<String>, Json(b): Json<SetSl>) -> ApiResult {
    let leg = st.legs.set_sl_percent(&id, b.percent).await?;
    Ok(Json(serde_json::to_value(leg).map_err(|e| Error::BadResponse(e.to_string()))?))
}

// ---------------------------------------------------------------- accounts

async fn list_accounts(State(st): State<AppState>) -> ApiResult {
    Ok(Json(json!({"accounts": st.accounts.list()})))
}

/// The saved secrets of one account, so the desk can show them when asked.
/// A POST, so nothing caches it and a stray link cannot fetch it.
async fn account_secrets(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(json!({"secrets": st.accounts.secrets(&id)?})))
}

async fn get_account(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(st.accounts.get(&id).ok_or_else(|| Error::NotFound(id))?))
}

/// Creates or updates an account. Secrets left blank keep their saved value.
async fn put_account(State(st): State<AppState>, Path(id): Path<String>, Json(b): Json<Upsert>) -> ApiResult {
    Ok(Json(st.accounts.upsert(&id, b)?))
}

async fn delete_account(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    st.accounts.remove(&id)?;
    Ok(Json(json!({"ok": true})))
}

/// Signs in every account marked auto-login, all at once. Always answers 200
/// with one result per account, so one bad password does not hide the others.
async fn login_all(State(st): State<AppState>) -> ApiResult {
    Ok(Json(json!({"results": st.accounts.login_all().await})))
}

/// Reads every signed-in account's margin again, all at once.
async fn refresh_funds(State(st): State<AppState>) -> ApiResult {
    Ok(Json(json!({"accounts": st.accounts.refresh_funds_all().await})))
}

async fn refresh_one_funds(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(st.accounts.refresh_funds(&id).await?))
}

/// Signs the account in now and answers with its state. On failure the error
/// says why (wrong password, bad TOTP secret, app not authorised...).
async fn login_account(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(st.accounts.login(&id).await?))
}

// ---------------------------------------------------------------- postback

/// Zerodha calls this when an order changes. The body is the raw JSON order;
/// `checksum` is SHA-256 of `order_id + order_timestamp + api_secret`. Without
/// the check, anyone who finds the URL could fake a fill.
async fn postback(State(st): State<AppState>, body: Bytes) -> StatusCode {
    let Some(secret) = &st.postback_secret else {
        return StatusCode::NOT_FOUND;
    };
    let Ok(v) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    let field = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("");
    let expected = hex::encode(Sha256::digest(
        format!("{}{}{}", field("order_id"), field("order_timestamp"), secret).as_bytes(),
    ));
    if !constant_time_eq(expected.as_bytes(), field("checksum").as_bytes()) {
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(order) = serde_json::from_value::<Order>(v) else {
        return StatusCode::BAD_REQUEST;
    };
    // Answer at once; Zerodha should not wait for an SL to be placed.
    let legs = st.legs.clone();
    tokio::spawn(async move { legs.on_update(&order).await });
    StatusCode::OK
}

// -------------------------------------------------------------- paper only

#[derive(Deserialize)]
struct PaperFill {
    order_id: String,
    price: f64,
    /// Fill only this many units (a partial fill). Omit to fill it all.
    #[serde(default)]
    quantity: Option<u32>,
}

/// Paper mode only: fill a resting order by hand, as the exchange would, and
/// tell the leg manager (as the real order stream would).
async fn paper_fill(State(st): State<AppState>, Json(b): Json<PaperFill>) -> ApiResult {
    let Some(paper) = &st.paper else {
        return Err(Error::NotFound("paper fills exist only in paper mode".into()).into());
    };
    match b.quantity {
        Some(q) => paper.simulate_partial(&b.order_id, q, b.price)?,
        None => paper.simulate_fill(&b.order_id, b.price)?,
    }
    if let Some(row) = st.engine.order_history(&b.order_id).await?.into_iter().last() {
        st.legs.on_update(&row).await;
    }
    Ok(Json(json!({"ok": true})))
}
