//! Kotak Neo, in two steps.
//!
//!   1. `tradeApiLogin`     mobile + UCC + TOTP  → a view token and a sid
//!   2. `tradeApiValidate`  the MPIN, with that view token → the trade token
//!
//! Only the trade token can trade. The view token looks like a working session
//! and quietly fails on anything that matters. Step 2 also returns the data
//! centre host this account must use for every later call.
//!
//! Kotak answers a failed login with HTTP 200 and `status: "error"`.

use super::*;
use serde_json::json;

async fn post(http: &reqwest::Client, url: &str, headers: &[(&str, &str)], body: Value) -> Result<Value> {
    let mut rb = http
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        // The same constant for every caller; omitting it reads like a bad credential.
        .header("neo-fin-key", "neotradeapi");
    for (k, v) in headers {
        rb = rb.header(*k, *v);
    }
    let res = rb.json(&body).send().await.map_err(|e| net("Kotak", e))?;
    let status = res.status().as_u16();
    let parsed = parse_json(&res.text().await.unwrap_or_default());
    if status >= 400 || str_at(&parsed, "status").eq_ignore_ascii_case("error") {
        let msg = error_message(&parsed, status, "Kotak");
        return Err(refused(status, format!("Kotak: {msg}{}", hint(&msg))));
    }
    Ok(parsed)
}

/// What usually causes Kotak's own refusals. The message is Kotak's; the advice
/// is where to look, since the request itself is the same one Kotak's official
/// client sends.
fn hint(msg: &str) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if m.contains("not registered") {
        ". Kotak has no TOTP registered for this exact UCC and mobile pair. Check: the UCC and mobile are this user's own; the Consumer key is from this same user's Neo API page (each user has their own); and the TOTP registration in the Neo app was finished by entering the first 6-digit code, not only scanning the QR"
    } else if m.contains("invalid totp") || m.contains("totp") && m.contains("invalid") {
        ". Check the TOTP secret is the key shown during Kotak's TOTP registration, and that this computer's clock is right"
    } else {
        ""
    }
}

pub async fn login(c: &Creds<'_>, urls: &BrokerUrls, opts: &KiteOptions) -> Result<BrokerSession> {
    let (consumer_key, pin, totp) = (c.secret("apiKey"), c.secret("pin"), c.secret("totpSecret"));
    let mobile = c.field("mobile");
    need(&[("UCC", c.user_id), ("Consumer key", consumer_key), ("Mobile", mobile), ("MPIN", pin), ("TOTP secret", totp)])?;
    crate::totp::validate_secret(totp).map_err(Error::Validation)?;

    let http = client(opts)?;
    let base = urls.kotak.trim_end_matches('/');
    let code = totp_code(totp, 0).await?;

    let first = data_of(
        &post(
            &http,
            &format!("{base}/login/1.0/tradeApiLogin"),
            &[("Authorization", consumer_key)],
            json!({ "mobileNumber": mobile, "ucc": c.user_id, "totp": code }),
        )
        .await?,
    );
    let (view_token, view_sid) = (str_at(&first, "token"), str_at(&first, "sid"));
    if view_token.is_empty() || view_sid.is_empty() {
        return Err(auth_err("Kotak login returned no view token or sid"));
    }

    let second = data_of(
        &post(
            &http,
            &format!("{base}/login/1.0/tradeApiValidate"),
            &[("Authorization", consumer_key), ("sid", &view_sid), ("Auth", &view_token)],
            json!({ "mpin": pin }),
        )
        .await?,
    );
    let trade_token = str_at(&second, "token");
    if trade_token.is_empty() {
        return Err(auth_err("Kotak MPIN check returned no trade token"));
    }
    let host = str_at(&second, "baseUrl").trim_end_matches('/').to_string();
    if host.is_empty() {
        return Err(auth_err("Kotak returned no data-centre address for this account, so nothing after sign-in can be reached"));
    }
    let sid = str_at(&second, "sid");

    let mut session = BrokerSession {
        access_token: trade_token,
        sid: if sid.is_empty() { view_sid } else { sid },
        base_url: host,
        client_code: c.user_id.to_string(),
        display_name: first_str(&second, &["greetingName", "clientName", "name"]),
        ..Default::default()
    };
    // Funds are best effort: a failed read does not undo the sign-in.
    session.funds = funds(&session, urls, opts).await.ok();
    Ok(session)
}

/// `POST <baseUrl>/quick/user/limits`: the account's margin, on the host sign-in
/// returned.
///
/// The request follows Kotak's docs: `Sid`, `Auth` and `neo-fin-key` headers and a
/// form body whose one field `jData` holds the JSON. Every amount comes back as
/// a string.
pub async fn funds(s: &BrokerSession, _urls: &BrokerUrls, opts: &KiteOptions) -> Result<Funds> {
    let http = client(opts)?;
    let jdata = json!({ "seg": "ALL", "exch": "ALL", "prod": "ALL" }).to_string();
    let url = format!("{}/quick/user/limits", s.base_url.trim_end_matches('/'));
    // This exact request has read a live account's margin, so it stays as it was.
    let ask = || {
        http.post(&url)
            .header("accept", "application/json")
            .header("Sid", &s.sid)
            .header("Auth", &s.access_token)
            .header("neo-fin-key", "neotradeapi")
            .form(&[("jData", jdata.as_str())])
            .send()
    };
    let mut res = ask().await.map_err(|e| net("Kotak limits", e))?;
    // Kotak's docs call a 424 "a temporary backend issue, retry after a few
    // seconds". One retry, a moment later, for that and for a 5xx.
    if res.status().as_u16() == 424 || res.status().is_server_error() {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        res = ask().await.map_err(|e| net("Kotak limits", e))?;
    }
    let status = res.status().as_u16();
    let text = res.text().await.unwrap_or_default();
    let mut v = parse_json(&text);
    if let Value::Array(a) = &v {
        v = a.first().cloned().unwrap_or(Value::Null);
    }
    // Some replies wrap the figures under `data`.
    if v.get("Net").is_none() {
        if let Some(d) = v.get("data").filter(|d| d.is_object()) {
            v = d.clone();
        }
    }
    let ok = str_at(&v, "stat").eq_ignore_ascii_case("ok") || v.get("stCode").and_then(Value::as_i64) == Some(200) || v.get("Net").is_some();
    if status == 424 || status >= 500 {
        return Err(Error::Network(format!(
            "Kotak's margin service is not answering right now (HTTP {status}). Kotak calls this a temporary problem on their side; it clears on its own. Use Refresh margins again in a few minutes"
        )));
    }
    if status >= 400 || !ok {
        let msg = first_str(&v, &["emsg", "message", "errMsg"]);
        let snippet: String = text.chars().take(160).collect();
        return Err(refused(status, format!("Kotak limits: {} (HTTP {status}{})", if msg.is_empty() { "no message" } else { msg.as_str() }, if msg.is_empty() { format!(", reply: {snippet}") } else { String::new() })));
    }
    let lines: [(&str, &str); 9] = [
        ("Adhoc margin", "AdhocMargin"),
        ("Notional cash", "NotionalCash"),
        ("SPAN", "SpanMarginPrsnt"),
        ("Exposure", "ExposureMarginPrsnt"),
        ("Premium", "PremiumPrsnt"),
        ("VAR margin", "MarginVarPrsnt"),
        ("Special margin", "SpecialMarginPrsnt"),
        ("Realised P&L", "RealizedMtomPrsnt"),
        ("Unrealised P&L", "UnrealizedMtomPrsnt"),
    ];
    let details = lines.iter().map(|(l, k)| (l.to_string(), num_at(&v, k))).filter(|(_, x)| *x != 0.0).collect();
    // `Net` is the margin available now, collateral included; `CollateralValue` is the pledged part.
    Ok(Funds::new(num_at(&v, "Net"), num_at(&v, "MarginUsed"), num_at(&v, "CollateralValue"), details))
}
