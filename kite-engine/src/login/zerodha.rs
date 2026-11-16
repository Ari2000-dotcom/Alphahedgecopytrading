//! Zerodha: Kite Connect has no login call, so the web sign-in is driven with a
//! cookie jar, then the request token is swapped for the day's access token.
//!
//!   1. GET  `/connect/login?api_key=…`               seeds cookies
//!   2. POST `/api/login`  user id + password         → request_id
//!   3. POST `/api/twofa`  the TOTP code              → cookies now authorised
//!   4. GET  `/connect/login…&skip_session=true`      → redirect with request_token
//!   5. POST `/session/token` with the checksum       → access_token
//!
//! Step 4 walks the redirects by hand and stops at the hop that carries the
//! request token. Following the last hop would deliver the token to the app's
//! registered redirect address, and Kite honours it once, so it would be spent
//! before step 5. The very first time an account connects to an app Kite parks
//! on an "Authorize" page that only a click clears; that case is reported.

use super::*;
use sha2::{Digest, Sha256};

fn request_token_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.query_pairs().find(|(k, _)| k == "request_token").map(|(_, v)| v.into_owned()))
        .unwrap_or_default()
}

/// Walks a redirect chain by hand, keeping cookies, stopping at a request token.
async fn jar_get(http: &reqwest::Client, jar: &mut Jar, start: &str) -> Result<(String, String)> {
    let mut url = start.to_string();
    for _ in 0..10 {
        let res = jar
            .with(http.get(&url).header("Accept", "text/html,application/xhtml+xml,*/*"))
            .send()
            .await
            .map_err(|e| net("Zerodha login page", e))?;
        jar.absorb(&res);
        let status = res.status();
        if status.is_redirection() {
            if let Some(loc) = res.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok()) {
                let base = url::Url::parse(&url).map_err(|e| Error::BadResponse(e.to_string()))?;
                url = base.join(loc).map_err(|e| Error::BadResponse(e.to_string()))?.to_string();
                let token = request_token_of(&url);
                if !token.is_empty() {
                    return Ok((url, token));
                }
                continue;
            }
        }
        if status.is_client_error() || status.is_server_error() {
            return Err(refused(status.as_u16(), format!("Zerodha login page returned HTTP {}", status.as_u16())));
        }
        let token = request_token_of(&url);
        return Ok((url, token));
    }
    Err(Error::Network("Zerodha login redirected too many times".into()))
}

async fn jar_post(http: &reqwest::Client, jar: &mut Jar, url: &str, form: &[(&str, &str)], referer: &str) -> Result<Value> {
    let res = jar
        .with(
            http.post(url)
                .header("Accept", "application/json")
                .header("X-Kite-Version", "3")
                .header("Referer", referer),
        )
        .form(form)
        .send()
        .await
        .map_err(|e| net("Zerodha", e))?;
    jar.absorb(&res);
    let status = res.status().as_u16();
    let env = parse_json(&res.text().await.unwrap_or_default());
    if status >= 400 || str_at(&env, "status").eq_ignore_ascii_case("error") {
        return Err(refused(status, format!("Zerodha: {}", error_message(&env, status, "Zerodha"))));
    }
    Ok(data_of(&env))
}

fn hint_totp(e: Error) -> Error {
    let m = e.to_string();
    let l = m.to_ascii_lowercase();
    if l.contains("totp") || l.contains("twofa") || l.contains("2fa") {
        auth_err(format!(
            "{m}. Check the TOTP secret is the key from Kite's External TOTP setup (\"Can't scan? Copy key\") and that this computer's clock is right"
        ))
    } else {
        e
    }
}

pub async fn login(c: &Creds<'_>, urls: &BrokerUrls, opts: &KiteOptions) -> Result<BrokerSession> {
    let (api_key, api_secret, password, totp) = (c.secret("apiKey"), c.secret("apiSecret"), c.secret("password"), c.secret("totpSecret"));
    need(&[("User ID", c.user_id), ("API key", api_key), ("API secret", api_secret), ("Web password", password), ("TOTP secret", totp)])?;
    crate::totp::validate_secret(totp).map_err(Error::Validation)?;

    let http = client(opts)?;
    let mut jar = Jar::default();
    let web = urls.zerodha_web.trim_end_matches('/');
    let api = urls.zerodha_api.trim_end_matches('/');
    let login_url = format!("{web}/connect/login?v=3&api_key={}", encode(api_key));

    // 1. seed cookies
    let (entry_url, _) = jar_get(&http, &mut jar, &login_url).await?;

    // 2. password
    let data = jar_post(&http, &mut jar, &format!("{web}/api/login"), &[("user_id", c.user_id), ("password", password)], &entry_url).await?;
    let request_id = str_at(&data, "request_id");
    if request_id.is_empty() {
        return Err(auth_err("Zerodha login did not return a request id"));
    }

    // 3. TOTP
    let twofa_type = str_at(&data, "twofa_type");
    let code = totp_code(totp, 0).await?;
    let mut form: Vec<(&str, &str)> = vec![("user_id", c.user_id), ("request_id", &request_id), ("twofa_value", &code)];
    if !twofa_type.is_empty() {
        form.push(("twofa_type", &twofa_type));
    }
    jar_post(&http, &mut jar, &format!("{web}/api/twofa"), &form, &entry_url).await.map_err(hint_totp)?;

    // 4. replay with skip_session so Kite mints a token instead of an "already logged in" page
    let mut replay = url::Url::parse(&entry_url).map_err(|e| Error::BadResponse(e.to_string()))?;
    replay.query_pairs_mut().append_pair("skip_session", "true");
    let (landed, request_token) = jar_get(&http, &mut jar, replay.as_str()).await?;
    if request_token.is_empty() {
        if landed.contains("/connect/authorize") || landed.contains("/connect/finish") {
            return Err(auth_err(format!(
                "Zerodha needs a one-time app authorization: open {login_url} in a browser, sign in and click Authorize once. Automatic sign-in works from then on"
            )));
        }
        return Err(auth_err(
            "Zerodha signed in but returned no request token: check the API key is active and its redirect URL matches the one in the Kite developer console",
        ));
    }

    // 5. exchange
    let checksum = hex::encode(Sha256::digest(format!("{api_key}{request_token}{api_secret}").as_bytes()));
    let res = http
        .post(format!("{api}/session/token"))
        .header("X-Kite-Version", "3")
        .header("Accept", "application/json")
        .form(&[("api_key", api_key), ("request_token", request_token.as_str()), ("checksum", checksum.as_str())])
        .send()
        .await
        .map_err(|e| net("Zerodha token exchange", e))?;
    let status = res.status().as_u16();
    let env = parse_json(&res.text().await.unwrap_or_default());
    let data = data_of(&env);
    let token = str_at(&data, "access_token");
    if status >= 400 || token.is_empty() {
        return Err(refused(status, format!("Zerodha token exchange failed: {}", error_message(&env, status, "Zerodha"))));
    }

    let mut session = BrokerSession {
        access_token: token,
        feed_token: str_at(&data, "public_token"),
        refresh_token: str_at(&data, "refresh_token"),
        base_url: api.to_string(),
        client_code: if str_at(&data, "user_id").is_empty() { c.user_id.to_string() } else { str_at(&data, "user_id") },
        display_name: str_at(&data, "user_name"),
        ..Default::default()
    };
    // Proves the token works and reads the funds; a failed read does not undo the sign-in.
    session.funds = funds(c, &session, urls, opts).await.ok();
    Ok(session)
}

/// `GET /user/margins`: equity and commodity, added together when both are enabled.
pub async fn funds(c: &Creds<'_>, s: &BrokerSession, urls: &BrokerUrls, opts: &KiteOptions) -> Result<Funds> {
    let http = client(opts)?;
    let api = urls.zerodha_api.trim_end_matches('/');
    let res = http
        .get(format!("{api}/user/margins"))
        .header("X-Kite-Version", "3")
        .header("Authorization", format!("token {}:{}", c.secret("apiKey"), s.access_token))
        .send()
        .await
        .map_err(|e| net("Zerodha margins", e))?;
    let status = res.status().as_u16();
    let env = parse_json(&res.text().await.unwrap_or_default());
    if status >= 400 || str_at(&env, "status").eq_ignore_ascii_case("error") {
        return Err(refused(status, format!("Zerodha margins: {}", error_message(&env, status, "Kite"))));
    }
    Ok(parse_margins(&data_of(&env)))
}

fn parse_margins(data: &Value) -> Funds {
    // A segment the account does not trade is reported with enabled: false.
    let segs: Vec<&Value> = ["equity", "commodity"]
        .iter()
        .filter_map(|k| data.get(*k))
        .filter(|v| v.get("enabled") != Some(&Value::Bool(false)))
        .collect();
    let sum = |group: &str, key: &str| segs.iter().map(|v| num_at(&v[group], key)).sum::<f64>();
    let lines: [(&str, &str, &str); 14] = [
        ("Opening balance", "available", "opening_balance"),
        ("Cash", "available", "cash"),
        ("Pay-in today", "available", "intraday_payin"),
        ("Adhoc margin", "available", "adhoc_margin"),
        ("SPAN", "utilised", "span"),
        ("Exposure", "utilised", "exposure"),
        ("Option premium", "utilised", "option_premium"),
        ("Realised P&L", "utilised", "m2m_realised"),
        ("Unrealised P&L", "utilised", "m2m_unrealised"),
        ("Holding sales", "utilised", "holding_sales"),
        ("Delivery", "utilised", "delivery"),
        ("Liquid collateral used", "utilised", "liquid_collateral"),
        ("Stock collateral used", "utilised", "stock_collateral"),
        ("Payout", "utilised", "payout"),
    ];
    let details = lines.iter().map(|(l, g, k)| (l.to_string(), sum(g, k))).filter(|(_, v)| *v != 0.0).collect();
    // `net` is the full available margin, pledged collateral included: the
    // figure Kite itself shows as available margin. `live_balance` is cash only.
    let net: f64 = segs.iter().map(|v| num_at(v, "net")).sum();
    Funds::new(net, sum("utilised", "debits"), sum("available", "collateral"), details).with_cash(sum("available", "live_balance"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_enabled_segments_and_skips_disabled_ones() {
        let d = serde_json::json!({
            "equity": {"enabled": true, "net": 1000.5,
                "available": {"live_balance": 900.5, "collateral": 100.0, "cash": 800.0, "opening_balance": 1000.0},
                "utilised": {"debits": 99.5, "span": 60.0, "exposure": 39.5}},
            "commodity": {"enabled": false, "net": 5000.0, "available": {"live_balance": 5000.0}, "utilised": {"debits": 7.0}},
        });
        let f = parse_margins(&d);
        // Available is net (cash plus collateral), not the cash-only live balance.
        assert_eq!((f.available, f.used, f.collateral, f.total), (1000.5, 99.5, 100.0, 1100.0));
        assert_eq!(f.cash, Some(900.5));
        assert!(f.details.contains(&("SPAN".to_string(), 60.0)));
        assert!(!f.details.iter().any(|(l, _)| l == "Payout"), "zero lines are left out");
    }
}
