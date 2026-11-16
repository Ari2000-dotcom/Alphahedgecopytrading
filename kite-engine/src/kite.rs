//! Zerodha Kite Connect v3 over HTTPS.
//!
//! Each client can be pinned to one outbound IP (`local_ip`) or sent through a
//! proxy, because Zerodha only accepts API orders from a registered static IP.

use crate::broker::Broker;
use crate::error::{Error, Result};
use crate::types::{ModifyRequest, Order, OrderRequest, Positions, Variety};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::net::IpAddr;
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://api.kite.trade";

#[derive(Debug, Clone, Default)]
pub struct KiteOptions {
    /// Override for tests. Defaults to the real Kite API.
    pub base_url: Option<String>,
    /// Send from this local address (a registered static IP on the VPS).
    pub local_ip: Option<IpAddr>,
    /// `http://user:pass@host:port` for a static-IP proxy service.
    pub proxy: Option<String>,
}

pub struct KiteClient {
    http: reqwest::Client,
    base: String,
    api_key: String,
    access_token: String,
}

// Never print the keys, even in a debug log.
impl std::fmt::Debug for KiteClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KiteClient").field("base", &self.base).field("keys", &"<redacted>").finish()
    }
}

#[derive(Deserialize)]
struct Envelope {
    status: Option<String>,
    data: Option<Value>,
    message: Option<String>,
    error_type: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Read,
    Write,
}

impl KiteClient {
    pub fn new(api_key: impl Into<String>, access_token: impl Into<String>) -> Result<Self> {
        Self::with_options(api_key, access_token, KiteOptions::default())
    }

    pub fn with_options(
        api_key: impl Into<String>,
        access_token: impl Into<String>,
        opts: KiteOptions,
    ) -> Result<Self> {
        let mut b = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            // Keeps the TLS connection warm so a fill-to-SL call skips the handshake.
            .tcp_keepalive(Duration::from_secs(30))
            .pool_idle_timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none());
        if let Some(ip) = opts.local_ip {
            b = b.local_address(ip);
        }
        if let Some(p) = &opts.proxy {
            // Don't echo the URL: it can carry the proxy password.
            let proxy = reqwest::Proxy::all(p).map_err(|_| Error::Validation("invalid proxy url".into()))?;
            b = b.proxy(proxy);
        }
        let http = b.build().map_err(|e| Error::Network(e.to_string()))?;
        Ok(Self {
            http,
            base: opts.base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()).trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            access_token: access_token.into(),
        })
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{}", self.base, path))
            .header("X-Kite-Version", "3")
            .header("Authorization", format!("token {}:{}", self.api_key, self.access_token))
    }

    async fn send(&self, rb: reqwest::RequestBuilder, kind: Kind) -> Result<Value> {
        let resp = rb.send().await.map_err(|e| {
            // A refused connection means nothing left this machine. Anything
            // else on a write (timeout, reset) could have reached the broker.
            if e.is_connect() || kind == Kind::Read {
                Error::Network(e.without_url().to_string())
            } else {
                Error::Unknown(e.without_url().to_string())
            }
        })?;
        let http_status = resp.status();
        let text = resp.text().await.map_err(|e| {
            if kind == Kind::Write {
                Error::Unknown(e.without_url().to_string())
            } else {
                Error::Network(e.without_url().to_string())
            }
        })?;
        let env: Envelope = serde_json::from_str(&text).map_err(|_| {
            let snippet: String = text.chars().take(200).collect();
            Error::BadResponse(format!("HTTP {http_status}: {snippet}"))
        })?;
        if env.status.as_deref() == Some("error") || !http_status.is_success() {
            return Err(Error::Api {
                kind: env.error_type.unwrap_or_else(|| format!("HTTP{}", http_status.as_u16())),
                message: env.message.unwrap_or_else(|| "no message".into()),
            });
        }
        env.data.ok_or_else(|| Error::BadResponse("reply had no data".into()))
    }

    fn order_id_of(data: &Value) -> Result<String> {
        data.get("order_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| Error::BadResponse("reply had no order_id".into()))
    }
}

fn push(form: &mut Vec<(&'static str, String)>, k: &'static str, v: Option<String>) {
    if let Some(v) = v {
        form.push((k, v));
    }
}

#[async_trait]
impl Broker for KiteClient {
    fn name(&self) -> &'static str {
        "zerodha"
    }

    async fn place_order(&self, r: &OrderRequest) -> Result<String> {
        r.validate()?;
        let mut f: Vec<(&'static str, String)> = vec![
            ("exchange", r.exchange.clone()),
            ("tradingsymbol", r.tradingsymbol.clone()),
            ("transaction_type", r.side.as_str().into()),
            ("order_type", r.order_type.as_str().into()),
            ("product", r.product.as_str().into()),
            ("quantity", r.quantity.to_string()),
        ];
        push(&mut f, "price", r.price.map(|p| p.to_string()));
        push(&mut f, "trigger_price", r.trigger_price.map(|p| p.to_string()));
        push(&mut f, "validity", r.validity.map(|v| v.as_str().into()));
        push(&mut f, "tag", r.tag.clone());
        push(&mut f, "market_protection", r.market_protection.map(|p| p.to_string()));
        let rb = self.req(reqwest::Method::POST, &format!("/orders/{}", r.variety.as_str())).form(&f);
        Self::order_id_of(&self.send(rb, Kind::Write).await?)
    }

    async fn modify_order(&self, variety: Variety, order_id: &str, m: &ModifyRequest) -> Result<String> {
        m.validate()?;
        let mut f: Vec<(&'static str, String)> = vec![];
        push(&mut f, "quantity", m.quantity.map(|v| v.to_string()));
        push(&mut f, "price", m.price.map(|v| v.to_string()));
        push(&mut f, "trigger_price", m.trigger_price.map(|v| v.to_string()));
        push(&mut f, "order_type", m.order_type.map(|v| v.as_str().into()));
        push(&mut f, "validity", m.validity.map(|v| v.as_str().into()));
        let rb = self
            .req(reqwest::Method::PUT, &format!("/orders/{}/{}", variety.as_str(), order_id))
            .form(&f);
        Self::order_id_of(&self.send(rb, Kind::Write).await?)
    }

    async fn cancel_order(&self, variety: Variety, order_id: &str) -> Result<String> {
        let rb = self.req(reqwest::Method::DELETE, &format!("/orders/{}/{}", variety.as_str(), order_id));
        Self::order_id_of(&self.send(rb, Kind::Write).await?)
    }

    async fn orders(&self) -> Result<Vec<Order>> {
        let data = self.send(self.req(reqwest::Method::GET, "/orders"), Kind::Read).await?;
        serde_json::from_value(data).map_err(|e| Error::BadResponse(e.to_string()))
    }

    async fn order_history(&self, order_id: &str) -> Result<Vec<Order>> {
        let data = self.send(self.req(reqwest::Method::GET, &format!("/orders/{order_id}")), Kind::Read).await?;
        serde_json::from_value(data).map_err(|e| Error::BadResponse(e.to_string()))
    }

    async fn positions(&self) -> Result<Positions> {
        let data = self.send(self.req(reqwest::Method::GET, "/portfolio/positions"), Kind::Read).await?;
        serde_json::from_value(data).map_err(|e| Error::BadResponse(e.to_string()))
    }

    async fn margins(&self) -> Result<Value> {
        self.send(self.req(reqwest::Method::GET, "/user/margins"), Kind::Read).await
    }
}
