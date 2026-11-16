//! Nubra: two calls.
//!
//!   1. `POST /totp/login`  phone + TOTP → an `auth_token`
//!   2. `POST /verifypin`   the MPIN, with that token → `session_token`
//!
//! The code is made from Nubra's own clock (its `Date` header), not this
//! computer's: a clock a few seconds out gives codes that fail as "invalid
//! TOTP", which sends people rotating a secret that was never wrong. Nubra also
//! wants the code as a number, and the same device id on every call.

use super::*;
use serde_json::json;

async fn call(
    http: &reqwest::Client,
    method: reqwest::Method,
    base: &str,
    path: &str,
    device: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Result<Value> {
    let mut rb = http.request(method, format!("{base}{path}")).header("Accept", "application/json").header("x-device-id", device);
    if let Some(t) = token {
        rb = rb.header("Authorization", format!("Bearer {t}"));
    }
    if let Some(b) = &body {
        rb = rb.json(b);
    }
    let res = rb.send().await.map_err(|e| net(&format!("Nubra {path}"), e))?;
    let status = res.status().as_u16();
    let parsed = parse_json(&res.text().await.unwrap_or_default());
    if status >= 400 {
        return Err(refused(status, format!("Nubra: {}", error_message(&parsed, status, "Nubra"))));
    }
    Ok(parsed)
}

/// How far Nubra's clock is from ours, in ms. Zero if it will not say.
async fn clock_offset_ms(http: &reqwest::Client, base: &str) -> i64 {
    let Ok(res) = http.head(format!("{base}/")).send().await else { return 0 };
    let Some(date) = res.headers().get(reqwest::header::DATE).and_then(|v| v.to_str().ok()) else { return 0 };
    let Ok(t) = httpdate::parse_http_date(date) else { return 0 };
    let server = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    // The Date header is whole seconds, so ignore a gap smaller than that.
    let gap = server - crate::totp::now_ms() as i64;
    if gap.abs() < 1_500 { 0 } else { gap }
}

pub async fn login(c: &Creds<'_>, urls: &BrokerUrls, opts: &KiteOptions) -> Result<BrokerSession> {
    let (pin, totp) = (c.secret("pin"), c.secret("totpSecret"));
    let phone = c.field("mobile");
    need(&[("Mobile", phone), ("MPIN", pin), ("TOTP secret", totp)])?;
    crate::totp::validate_secret(totp).map_err(Error::Validation)?;

    let http = client(opts)?;
    let base = urls.nubra.trim_end_matches('/');
    let alnum: String = phone.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let device = format!("alphahedge-{alnum}");

    let code = totp_code(totp, clock_offset_ms(&http, base).await).await?;
    // A NUMBER, not a string: "042311" is rejected as an invalid code.
    let code_num: u32 = code.parse().map_err(|_| auth_err("the TOTP code was not numeric"))?;

    let first = call(
        &http, reqwest::Method::POST, base, "/totp/login", &device, None,
        Some(json!({ "phone": phone, "totp": code_num, "otp": "" })),
    )
    .await
    .map_err(|e| {
        let m = e.to_string();
        let l = m.to_ascii_lowercase();
        if l.contains("totp") || l.contains("otp") || l.contains("invalid") {
            auth_err(format!("{m}. Check the TOTP secret matches the one enabled on this Nubra account and this computer's clock is right"))
        } else {
            e
        }
    })?;
    let auth_token = str_at(&first, "auth_token");
    if auth_token.is_empty() {
        return Err(auth_err("Nubra TOTP login returned no auth token"));
    }

    let verified = call(&http, reqwest::Method::POST, base, "/verifypin", &device, Some(&auth_token), Some(json!({ "pin": pin }))).await?;
    let session_token = str_at(&verified, "session_token");
    if session_token.is_empty() {
        return Err(auth_err("Nubra MPIN check returned no session token"));
    }

    // Who this is. Not fatal: the session works either way.
    let mut s = BrokerSession {
        access_token: session_token.clone(),
        base_url: base.to_string(),
        device_id: device.clone(),
        client_code: c.user_id.to_string(),
        ..Default::default()
    };
    if let Ok(info) = call(&http, reqwest::Method::GET, base, "/users/account_info", &device, Some(&session_token), None).await {
        let d = data_of(&info);
        let code = first_str(&d, &["client_code", "clientCode", "ucc"]);
        if !code.is_empty() {
            s.client_code = code;
        }
        s.display_name = first_str(&d, &["name", "client_name", "display_name"]);
    }
    Ok(s)
}
