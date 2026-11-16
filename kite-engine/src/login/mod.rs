//! Automatic sign-in for each broker, ported from the proven adapters in
//! `D:\Alphahedgedesk\backend\rust\src\brokers\adapters`.
//!
//! Every broker reduces to the same shape: saved details in, a `BrokerSession`
//! out. Angel, Kotak and Nubra are plain TOTP calls. Zerodha and Upstox have no
//! documented login call, so their own sign-in pages are driven the way a
//! browser drives them; that can break when the broker changes the page, and
//! when it does the error says which step failed.
//!
//! Nothing here retries by itself. A wrong password retried is a locked account,
//! so `accounts.rs` decides when to try again, and gives up after two failures.

pub mod angel;
pub mod kotak;
pub mod nubra;
pub mod upstox;
pub mod zerodha;

use crate::error::{Error, Result};
use crate::kite::KiteOptions;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

/// Every address a login talks to. Tests point them at a local mock.
#[derive(Debug, Clone)]
pub struct BrokerUrls {
    pub zerodha_web: String,
    pub zerodha_api: String,
    pub angel: String,
    /// Kotak's login host (the later calls use a per-account host it returns).
    pub kotak: String,
    pub nubra: String,
    /// Upstox's OAuth host, with the `/v2` path.
    pub upstox_api: String,
    /// The host behind login.upstox.com.
    pub upstox_login: String,
}

impl Default for BrokerUrls {
    fn default() -> Self {
        Self {
            zerodha_web: "https://kite.zerodha.com".into(),
            zerodha_api: crate::kite::DEFAULT_BASE_URL.into(),
            angel: "https://apiconnect.angelone.in".into(),
            kotak: "https://mis.kotaksecurities.com".into(),
            nubra: "https://api.nubra.io".into(),
            upstox_api: "https://api.upstox.com/v2".into(),
            upstox_login: "https://service.upstox.com".into(),
        }
    }
}

impl BrokerUrls {
    /// Every broker on one local mock server (tests).
    pub fn all_at(base: &str) -> Self {
        let b = base.trim_end_matches('/');
        Self {
            zerodha_web: b.into(),
            zerodha_api: b.into(),
            angel: b.into(),
            kotak: b.into(),
            nubra: b.into(),
            upstox_api: format!("{b}/v2"),
            upstox_login: b.into(),
        }
    }
}

/// What a sign-in needs: the account's saved details.
pub struct Creds<'a> {
    pub user_id: &'a str,
    pub secrets: &'a BTreeMap<String, String>,
    pub fields: &'a BTreeMap<String, String>,
}

impl Creds<'_> {
    pub fn secret(&self, k: &str) -> &str {
        self.secrets.get(k).map(|s| s.trim()).unwrap_or("")
    }
    pub fn field(&self, k: &str) -> &str {
        self.fields.get(k).map(|s| s.trim()).unwrap_or("")
    }
}

/// A signed-in broker session. The tokens are not interchangeable: Angel's
/// WebSocket needs `feed_token`, never the REST token, and Kotak's calls need
/// `sid` and the per-account `base_url` it returns.
#[derive(Clone, Default)]
pub struct BrokerSession {
    pub access_token: String,
    pub feed_token: String,
    pub refresh_token: String,
    pub sid: String,
    pub base_url: String,
    pub device_id: String,
    pub client_code: String,
    pub display_name: String,
    /// Funds, read right after sign-in when the broker allows it.
    pub funds: Option<Funds>,
}

/// An account's margin in the same terms for every broker.
///
/// `available` is the full margin that can be used now, collateral from pledged
/// holdings included, as the broker's own app shows it. `used` is margin
/// already blocked, `total` the two added up. `cash` is the cash part on its
/// own and `collateral` the pledged part, where the broker reports them.
/// `details` carries the broker's further lines (span, exposure, premium,
/// P&L...) as label and amount, in display order, so nothing is hidden.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Funds {
    pub available: f64,
    pub used: f64,
    pub collateral: f64,
    pub total: f64,
    pub cash: Option<f64>,
    pub details: Vec<(String, f64)>,
}

impl Funds {
    pub fn new(available: f64, used: f64, collateral: f64, details: Vec<(String, f64)>) -> Self {
        Self { available, used, collateral, total: available + used, cash: None, details }
    }

    /// The cash part, for brokers that report it separately.
    pub fn with_cash(mut self, cash: f64) -> Self {
        self.cash = Some(cash);
        self
    }
}

/// A number from a field that may be a JSON number or a numeric string
/// (Angel and Kotak send amounts as strings).
pub(crate) fn num_at(v: &Value, key: &str) -> f64 {
    match v.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().replace(',', "").parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Reads the account's current margin from its broker. Needs a live session.
pub async fn funds(broker: &str, c: &Creds<'_>, s: &BrokerSession, urls: &BrokerUrls, opts: &KiteOptions) -> Result<Funds> {
    match broker {
        "zerodha" => zerodha::funds(c, s, urls, opts).await,
        "angel" => angel::funds(c, s, urls, opts).await,
        "kotak" => kotak::funds(s, urls, opts).await,
        "upstox" => upstox::funds(s, urls, opts).await,
        other => Err(Error::Validation(format!("{other} does not report funds yet"))),
    }
}

// Never print a token, even in a debug log.
impl std::fmt::Debug for BrokerSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrokerSession").field("client_code", &self.client_code).field("tokens", &"<redacted>").finish()
    }
}

/// Signs in to whichever broker this is.
pub async fn login(broker: &str, c: &Creds<'_>, urls: &BrokerUrls, opts: &KiteOptions) -> Result<BrokerSession> {
    match broker {
        "zerodha" => zerodha::login(c, urls, opts).await,
        "angel" => angel::login(c, urls, opts).await,
        "kotak" => kotak::login(c, urls, opts).await,
        "nubra" => nubra::login(c, urls, opts).await,
        "upstox" => upstox::login(c, urls, opts).await,
        other => Err(Error::Validation(format!("{other} cannot be signed in"))),
    }
}

// ---------------------------------------------------------------- helpers

pub(crate) const BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

/// One HTTP client. Redirects are never followed by it: Zerodha's and Upstox's
/// chains must be walked by hand, to stop at the hop that carries the token
/// (following the last hop would spend the one-time token on the app's own
/// redirect address).
pub(crate) fn client(opts: &KiteOptions) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .user_agent(BROWSER_UA)
        .redirect(reqwest::redirect::Policy::none());
    if let Some(ip) = opts.local_ip {
        b = b.local_address(ip);
    }
    if let Some(p) = &opts.proxy {
        // Don't echo the URL: it can carry the proxy password.
        b = b.proxy(reqwest::Proxy::all(p).map_err(|_| Error::Validation("invalid proxy url".into()))?);
    }
    b.build().map_err(|e| Error::Network(e.without_url().to_string()))
}

pub(crate) fn net(what: &str, e: reqwest::Error) -> Error {
    Error::Network(format!("{what}: {}", e.without_url()))
}

/// A refusal at login: 5xx is the broker having a bad minute, anything else is
/// the details or the account, which trying again cannot fix.
pub(crate) fn refused(status: u16, message: String) -> Error {
    if status >= 500 {
        Error::Network(message)
    } else {
        Error::Api { kind: "login".into(), message }
    }
}

pub(crate) fn auth_err(message: impl Into<String>) -> Error {
    Error::Api { kind: "login".into(), message: message.into() }
}

/// Every missing detail, by name, before any request is made.
pub(crate) fn need(items: &[(&str, &str)]) -> Result<()> {
    let missing: Vec<&str> = items.iter().filter(|(_, v)| v.trim().is_empty()).map(|(k, _)| *k).collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(Error::Validation(format!("{} not saved for this account", missing.join(", ") + if missing.len() == 1 { " is" } else { " are" })))
    }
}

pub(crate) fn parse_json(text: &str) -> Value {
    if text.trim().is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(text).unwrap_or_else(|_| {
        let snippet: String = text.chars().take(200).collect();
        serde_json::json!({ "message": snippet })
    })
}

/// `value[key]` as a trimmed string; empty when absent or not a scalar.
pub(crate) fn str_at(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

pub(crate) fn first_str(v: &Value, keys: &[&str]) -> String {
    keys.iter().map(|k| str_at(v, k)).find(|s| !s.is_empty()).unwrap_or_default()
}

/// Brokers nest the payload under `data` on one reply and not on the next.
pub(crate) fn data_of(v: &Value) -> Value {
    match v.get("data") {
        Some(d) if d.is_object() => d.clone(),
        _ => v.clone(),
    }
}

/// The broker's own words for a failure, whichever key it used.
pub(crate) fn error_message(body: &Value, status: u16, fallback: &str) -> String {
    for key in ["message", "error", "detail", "errorMessage"] {
        let v = str_at(body, key);
        if !v.is_empty() {
            return v;
        }
    }
    if let Some(Value::Array(errors)) = body.get("error") {
        if let Some(first) = errors.first() {
            let v = str_at(first, "message");
            if !v.is_empty() {
                return v;
            }
        }
    }
    if let Some(fault) = body.get("fault") {
        let v = str_at(fault, "message");
        if !v.is_empty() {
            return v;
        }
    }
    format!("{fallback} HTTP {status}")
}

/// A deliberately minimal cookie jar: name to value, no domain or path rules.
/// Zerodha's and Upstox's sign-in thread their session through cookies across a
/// few hosts of the same broker, and this is the behaviour the proven adapters
/// rely on.
#[derive(Default)]
pub(crate) struct Jar(BTreeMap<String, String>);

impl Jar {
    pub fn header(&self) -> String {
        self.0.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
    }

    pub fn absorb(&mut self, res: &reqwest::Response) {
        for value in res.headers().get_all(reqwest::header::SET_COOKIE) {
            let Ok(line) = value.to_str() else { continue };
            let pair = line.split(';').next().unwrap_or_default();
            if let Some((name, value)) = pair.split_once('=') {
                self.0.insert(name.trim().to_string(), value.trim().to_string());
            }
        }
    }

    pub fn with(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let c = self.header();
        if c.is_empty() {
            rb
        } else {
            rb.header(reqwest::header::COOKIE, c)
        }
    }
}

pub(crate) fn encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// A TOTP code, mapped into the engine's error type.
pub(crate) async fn totp_code(secret: &str, offset_ms: i64) -> Result<String> {
    crate::totp::safe_code(secret, offset_ms).await.map_err(Error::Validation)
}
