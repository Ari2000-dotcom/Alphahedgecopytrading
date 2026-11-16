//! Angel One (SmartAPI): one TOTP call, three tokens.
//!
//!   `jwtToken`      REST calls, as `Authorization: Bearer <jwt>`
//!   `refreshToken`  a new jwt without running TOTP again
//!   `feedToken`     the WebSocket, and only the WebSocket
//!
//! SmartAPI answers a failed call with HTTP 200 and `status: false`, so the
//! body's `status` is what is checked, not the HTTP status.

use super::*;
use serde_json::json;

const LOGIN_PATH: &str = "/rest/auth/angelbroking/user/v1/loginByPassword";
const RMS_PATH: &str = "/rest/secure/angelbroking/user/v1/getRMS";

/// SmartAPI wants client-identity headers. They must be present and well
/// formed, but are not validated, so loopback values are sent rather than
/// handing the broker this computer's real addresses.
fn smart_headers(rb: reqwest::RequestBuilder, api_key: &str) -> reqwest::RequestBuilder {
    rb.header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-UserType", "USER")
        .header("X-SourceID", "WEB")
        .header("X-ClientLocalIP", "127.0.0.1")
        .header("X-ClientPublicIP", "127.0.0.1")
        .header("X-MACAddress", "00:00:00:00:00:00")
        .header("X-PrivateKey", api_key)
}

pub async fn login(c: &Creds<'_>, urls: &BrokerUrls, opts: &KiteOptions) -> Result<BrokerSession> {
    let (api_key, pin, totp) = (c.secret("apiKey"), c.secret("pin"), c.secret("totpSecret"));
    need(&[("Client code", c.user_id), ("API key", api_key), ("PIN", pin), ("TOTP secret", totp)])?;
    crate::totp::validate_secret(totp).map_err(Error::Validation)?;

    let http = client(opts)?;
    let base = urls.angel.trim_end_matches('/');
    let code = totp_code(totp, 0).await?;

    let res = smart_headers(http.post(format!("{base}{LOGIN_PATH}")), api_key)
        .json(&json!({ "clientcode": c.user_id, "password": pin, "totp": code }))
        .send()
        .await
        .map_err(|e| net("Angel login", e))?;
    let status = res.status().as_u16();
    let env = parse_json(&res.text().await.unwrap_or_default());
    if status >= 400 || !env.get("status").and_then(Value::as_bool).unwrap_or(status < 400) {
        let errcode = str_at(&env, "errorcode");
        let msg = error_message(&env, status, "SmartAPI");
        return Err(refused(status, if errcode.is_empty() { format!("Angel: {msg}") } else { format!("Angel: {msg} [{errcode}]") }));
    }
    let data = data_of(&env);
    let jwt = str_at(&data, "jwtToken");
    if jwt.is_empty() {
        return Err(auth_err("Angel login returned no jwtToken"));
    }

    let mut session = BrokerSession {
        access_token: jwt,
        feed_token: str_at(&data, "feedToken"),
        refresh_token: str_at(&data, "refreshToken"),
        base_url: base.to_string(),
        client_code: c.user_id.to_string(),
        ..Default::default()
    };
    // Funds, and proof the token works. A failure here does not undo the sign-in.
    session.funds = funds(c, &session, urls, opts).await.ok();
    Ok(session)
}

/// `GET getRMS`: the account's margin.
///
/// `net` is what can be used now, `utiliseddebits` what is already blocked.
/// Angel sends every amount as a string.
pub async fn funds(c: &Creds<'_>, s: &BrokerSession, urls: &BrokerUrls, opts: &KiteOptions) -> Result<Funds> {
    let http = client(opts)?;
    let base = urls.angel.trim_end_matches('/');
    let res = smart_headers(http.get(format!("{base}{RMS_PATH}")), c.secret("apiKey"))
        .header("Authorization", format!("Bearer {}", s.access_token))
        .send()
        .await
        .map_err(|e| net("Angel funds", e))?;
    let status = res.status().as_u16();
    let env = parse_json(&res.text().await.unwrap_or_default());
    if status >= 400 || !env.get("status").and_then(Value::as_bool).unwrap_or(status < 400) {
        return Err(refused(status, format!("Angel funds: {}", error_message(&env, status, "SmartAPI"))));
    }
    let d = data_of(&env);
    let lines: [(&str, &str); 11] = [
        ("Cash", "availablecash"),
        ("Pay-in today", "availableintradaypayin"),
        ("Limit margin", "availablelimitmargin"),
        ("SPAN", "utilisedspan"),
        ("Exposure", "utilisedexposure"),
        ("Option premium", "utilisedoptionpremium"),
        ("Realised P&L", "m2mrealized"),
        ("Unrealised P&L", "m2munrealized"),
        ("Holding sales", "utilisedholdingsales"),
        ("Turnover", "utilisedturnover"),
        ("Payout", "utilisedpayout"),
    ];
    let details = lines.iter().map(|(l, k)| (l.to_string(), num_at(&d, k))).filter(|(_, v)| *v != 0.0).collect();
    Ok(Funds::new(num_at(&d, "net"), num_at(&d, "utiliseddebits"), num_at(&d, "collateral"), details).with_cash(num_at(&d, "availablecash")))
}
