//! Kite's order stream: `wss://ws.kite.trade?api_key=...&access_token=...`.
//!
//! Order updates arrive as text frames `{"type":"order","data":{...}}`, the same
//! payload as a postback. Binary frames are market data or a 1-byte heartbeat;
//! this socket subscribes to no instruments, so they are only a sign of life.
//!
//! If nothing at all arrives for `idle_timeout` the link is treated as dead and
//! reopened. After every (re)connect the order book is read once, so anything
//! that happened while the link was down is applied.

use crate::leg::LegManager;
use crate::types::Order;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone)]
pub struct WsConfig {
    /// Includes the keys: never log it.
    pub url: String,
    pub idle_timeout: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
}

impl WsConfig {
    pub fn kite(api_key: &str, access_token: &str) -> Self {
        Self {
            url: format!("wss://ws.kite.trade?api_key={api_key}&access_token={access_token}"),
            idle_timeout: Duration::from_secs(15),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
        }
    }
}

/// Parse one text frame. `Some` only for an order update.
pub fn parse_frame(text: &str) -> Option<Order> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    match v.get("type")?.as_str()? {
        "order" => serde_json::from_value(v.get("data")?.clone()).ok(),
        "error" => {
            tracing::warn!("order stream error message: {}", v.get("data").map(|d| d.to_string()).unwrap_or_default());
            None
        }
        _ => None,
    }
}

pub async fn run_order_stream(cfg: WsConfig, mgr: Arc<LegManager>, mut stop: watch::Receiver<bool>) {
    let mut backoff = cfg.backoff_min;
    loop {
        if *stop.borrow() {
            return;
        }
        match tokio_tungstenite::connect_async(cfg.url.as_str()).await {
            Ok((mut stream, _)) => {
                tracing::info!("order stream connected");
                backoff = cfg.backoff_min;
                // Catch up on anything missed while we were away.
                mgr.reconcile_all().await;
                loop {
                    tokio::select! {
                        _ = stop.changed() => return,
                        next = tokio::time::timeout(cfg.idle_timeout, stream.next()) => match next {
                            Err(_) => { tracing::warn!("order stream silent for {:?}; reconnecting", cfg.idle_timeout); break; }
                            Ok(None) => { tracing::warn!("order stream closed; reconnecting"); break; }
                            Ok(Some(Err(e))) => { tracing::warn!("order stream error: {e}; reconnecting"); break; }
                            Ok(Some(Ok(Message::Text(t)))) => {
                                if let Some(order) = parse_frame(t.as_str()) {
                                    // Handle off the read loop so a slow SL call
                                    // never delays reading the next frame.
                                    let m = mgr.clone();
                                    tokio::spawn(async move { m.on_update(&order).await });
                                }
                            }
                            Ok(Some(Ok(Message::Close(_)))) => { tracing::warn!("order stream closed by server"); break; }
                            Ok(Some(Ok(_))) => {}
                        }
                    }
                }
            }
            // Do not print the URL: it carries the keys.
            Err(e) => tracing::warn!("order stream cannot connect: {e}"),
        }
        tokio::select! {
            _ = stop.changed() => return,
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(cfg.backoff_max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_order_frames_only() {
        let f = r#"{"type":"order","data":{"order_id":"9","status":"COMPLETE","filled_quantity":75,"average_price":100.5,"tag":"L1"}}"#;
        let o = parse_frame(f).unwrap();
        assert_eq!((o.order_id.as_str(), o.status.as_str(), o.filled_quantity), ("9", "COMPLETE", 75));
        assert_eq!(o.average_price, 100.5);
        assert!(parse_frame(r#"{"type":"message","data":"hi"}"#).is_none());
        assert!(parse_frame(r#"{"type":"error","data":"bad"}"#).is_none());
        assert!(parse_frame("not json").is_none());
    }
}
