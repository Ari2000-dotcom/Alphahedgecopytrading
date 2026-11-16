//! Upstox: OAuth2, driven headlessly.
//!
//! Upstox publishes only the browser flow, and the token it gives expires about
//! 03:30 IST the next morning. The sign-in page is a thin client over a private
//! JSON API, and every step is a request this process can make, including the
//! second factor (a TOTP). So the calls the browser makes are made here:
//!
//!   1. GET  `/login/authorization/dialog`           → redirect carrying `user_id`
//!   2. POST `/login/open/v6/auth/1fa/otp/generate`  → `validateOTPToken`
//!   3. POST `/login/open/v4/auth/1fa/otp-totp/verify` with a generated TOTP
//!   4. POST `/login/open/v3/auth/2fa` with the PIN (base64)
//!   5. POST `/login/v2/oauth/authorize`  (only the first time an app is approved)
//!
//! and then the documented code-for-token exchange. Steps 2 to 5 are
//! undocumented, so they can change without notice. The account must use TOTP
//! (not SMS OTP) as its second factor, and the redirect URL must match the one
//! in the Upstox developer console exactly.

use super::*;
use base64::Engine as _;
use serde_json::json;

const ORIGIN: &str = "https://login.upstox.com";

/// The browser address for a manual sign-in, quoted in errors as the fallback.
fn dialog_url(api: &str, api_key: &str, redirect: &str) -> String {
    format!("{api}/login/authorization/dialog?client_id={}&redirect_uri={}&response_type=code", encode(api_key), encode(redirect))
}

/// The `code` out of a redirect address; empty when there is none.
fn code_of(redirect_uri: &str) -> String {
    url::Url::parse(redirect_uri)
        .ok()
        .and_then(|u| u.query_pairs().find(|(k, _)| k == "code").map(|(_, v)| v.into_owned()))
        .unwrap_or_default()
}

/// Different per login: step 5 is addressed by it.
fn request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut n = crate::totp::now_ms() ^ (SEQ.fetch_add(1, Ordering::Relaxed) << 44);
    let mut out = String::from("WPRO-");
    for _ in 0..10 {
        out.push(A[(n % 36) as usize] as char);
        n /= 36;
    }
    out
}

/// What an Upstox error code usually means. `UDAPI100069` says "check your
/// client_id and client_secret" and is wrong more often than right.
fn explain(code: &str) -> Option<&'static str> {
    match code {
        "UDAPI100069" => Some("check the API secret is complete and current, the redirect URL matches the Upstox console exactly, and the sign-in code was not already used. Upstox returns this one code for all of them"),
        "UDAPI100057" => Some("the access token has expired: sign in again"),
        "UDAPI100050" => Some("the redirect URL does not match the one registered in the Upstox console"),
        _ => None,
    }
}

fn upstox_error(body: &Value, status: u16) -> String {
    let nested = match (body.get("errors"), body.get("error")) {
        (Some(Value::Array(e)), _) => e.first().cloned(),
        (_, Some(one @ Value::Object(_))) => Some(one.clone()),
        _ => None,
    };
    if let Some(first) = nested {
        let message = first_str(&first, &["message", "errorMessage"]);
        let code = first_str(&first, &["errorCode", "error_code"]);
        if !message.is_empty() {
            let base = if code.is_empty() { message } else { format!("{message} [{code}]") };
            return match explain(&code) {
                Some(h) => format!("{base}: {h}"),
                None => base,
            };
        }
    }
    error_message(body, status, "Upstox")
}

async fn login_post(http: &reqwest::Client, jar: &mut Jar, url: &str, query: &[(&str, &str)], body: Value, step: &str) -> Result<Value> {
    let res = jar
        .with(
            http.post(url)
                .query(query)
                .header("accept", "*/*")
                .header("content-type", "application/json")
                .header("origin", ORIGIN)
                .header("referer", format!("{ORIGIN}/"))
                .header("x-request-id", request_id())
                .header("x-device-details", "platform=WEB|osName=Windows/10|osVersion=10|appVersion=4.0.0|modelName=Chrome|manufacturer=Google"),
        )
        .json(&body)
        .send()
        .await
        .map_err(|e| net(&format!("Upstox {step}"), e))?;
    jar.absorb(&res);
    let status = res.status().as_u16();
    let payload = parse_json(&res.text().await.unwrap_or_default());
    if status >= 400 || payload.get("success") == Some(&Value::Bool(false)) {
        return Err(refused(status, format!("Upstox {step}: {}", upstox_error(&payload, status))));
    }
    Ok(data_of(&payload))
}

/// Step 1: the dialog redirects to the sign-in page carrying the `user_id`.
async fn dialog_user_id(http: &reqwest::Client, jar: &mut Jar, start: &str) -> Result<String> {
    let mut url = start.to_string();
    for _ in 0..8 {
        let res = jar
            .with(http.get(&url).header("Accept", "text/html,application/xhtml+xml,*/*"))
            .send()
            .await
            .map_err(|e| net("Upstox authorization dialog", e))?;
        jar.absorb(&res);
        let status = res.status();
        let loc = res.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok()).map(str::to_string);
        match loc {
            Some(loc) if status.is_redirection() => {
                let base = url::Url::parse(&url).map_err(|e| Error::BadResponse(e.to_string()))?;
                let next = base.join(&loc).map_err(|e| Error::BadResponse(e.to_string()))?;
                if let Some((_, id)) = next.query_pairs().find(|(k, _)| k == "user_id") {
                    if !id.trim().is_empty() {
                        return Ok(id.into_owned());
                    }
                }
                url = next.to_string();
            }
            _ => {
                if status.as_u16() >= 400 {
                    return Err(auth_err(format!(
                        "Upstox authorization dialog returned HTTP {}: check the API key is active and the redirect URL matches the developer console exactly",
                        status.as_u16()
                    )));
                }
                break;
            }
        }
    }
    Err(auth_err("Upstox never returned a user id for this API key: the key or the registered redirect URL is wrong"))
}

pub async fn login(c: &Creds<'_>, urls: &BrokerUrls, opts: &KiteOptions) -> Result<BrokerSession> {
    let (api_key, api_secret, pin, totp) = (c.secret("apiKey"), c.secret("apiSecret"), c.secret("pin"), c.secret("totpSecret"));
    let (redirect, phone) = (c.field("redirectUrl"), c.field("mobile"));
    need(&[("API key", api_key), ("API secret", api_secret), ("Redirect URL", redirect), ("Mobile", phone), ("PIN", pin), ("TOTP secret", totp)])?;
    crate::totp::validate_secret(totp).map_err(Error::Validation)?;

    let api = urls.upstox_api.trim_end_matches('/');
    let svc = urls.upstox_login.trim_end_matches('/');
    let http = client(opts)?;
    let mut jar = Jar::default();
    let browser_hint = format!("If it keeps failing, sign in once in a browser: {}", dialog_url(api, api_key, redirect));
    let wrap = |e: Error| auth_err(format!("Upstox unattended sign-in failed: {e}. {browser_hint}"));

    let user_id = dialog_user_id(&http, &mut jar, &dialog_url(api, api_key, redirect)).await.map_err(wrap)?;

    // 2. ask for the second factor; the reply says which one the account uses
    let otp = login_post(
        &http, &mut jar, &format!("{svc}/login/open/v6/auth/1fa/otp/generate"), &[],
        json!({ "data": { "mobileNumber": phone, "userId": user_id } }), "OTP request",
    )
    .await
    .map_err(wrap)?;
    if otp.get("isTotpEnabled") == Some(&Value::Bool(false)) {
        return Err(auth_err("This Upstox account still uses SMS OTP as its second factor. Turn on TOTP in the Upstox app so it can sign in unattended"));
    }
    let validate = first_str(&otp, &["validateOTPToken", "validateOtpToken"]);
    if validate.is_empty() {
        return Err(wrap(auth_err("Upstox accepted the mobile number but returned no OTP token")));
    }

    // 3. TOTP
    let code = totp_code(totp, 0).await?;
    login_post(
        &http, &mut jar, &format!("{svc}/login/open/v4/auth/1fa/otp-totp/verify"), &[],
        json!({ "data": { "otp": code, "validateOtpToken": validate } }), "TOTP",
    )
    .await
    .map_err(wrap)?;

    // 4. PIN. From here the query carries the OAuth app, because what comes
    //    back is a callback address for it.
    let app_query = [("client_id", api_key), ("redirect_uri", redirect)];
    let twofa = login_post(
        &http, &mut jar, &format!("{svc}/login/open/v3/auth/2fa"), &app_query,
        json!({ "data": { "twoFAMethod": "SECRET_PIN", "inputText": base64::engine::general_purpose::STANDARD.encode(pin.as_bytes()) } }),
        "PIN",
    )
    .await
    .map_err(wrap)?;

    // 5. an app approved before comes back with the code attached; a first
    //    connection needs the approval posted. Testing for the code, not for
    //    Upstox's own flag, means a change to that flag cannot cost a sign-in.
    let mut code = code_of(&str_at(&twofa, "redirectUri"));
    if code.is_empty() {
        let rid = request_id();
        let approval = login_post(
            &http, &mut jar, &format!("{svc}/login/v2/oauth/authorize"),
            &[("client_id", api_key), ("redirect_uri", redirect), ("requestId", &rid), ("response_type", "code")],
            json!({ "data": { "userOAuthApproval": true } }), "authorization",
        )
        .await
        .map_err(wrap)?;
        code = code_of(&str_at(&approval, "redirectUri"));
    }
    if code.is_empty() {
        return Err(wrap(auth_err("Upstox signed in but returned no authorization code: check the redirect URL matches the Upstox console exactly")));
    }

    // The documented exchange, with the exact redirect URL used above.
    let res = http
        .post(format!("{api}/login/authorization/token"))
        .header("accept", "application/json")
        .form(&[
            ("code", code.as_str()), ("client_id", api_key), ("client_secret", api_secret),
            ("redirect_uri", redirect), ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| net("Upstox token exchange", e))?;
    let status = res.status().as_u16();
    let body = parse_json(&res.text().await.unwrap_or_default());
    // Flat on success; wrapped under `data` on some errors.
    let payload = if body.get("access_token").is_some() { body.clone() } else { data_of(&body) };
    let token = str_at(&payload, "access_token");
    if status >= 400 || token.is_empty() {
        let mut why = upstox_error(&body, status);
        if api_secret.len() < 16 {
            why.push_str(&format!(". The API secret is only {} characters, which is suspiciously short: copy it again from the Upstox developer console", api_secret.len()));
        }
        return Err(refused(status, format!("Upstox token exchange failed: {why}")));
    }

    let mut session = BrokerSession {
        access_token: token,
        base_url: api.to_string(),
        client_code: first_str(&payload, &["user_id", "userId"]),
        display_name: first_str(&payload, &["user_name", "userName", "email"]),
        ..Default::default()
    };
    // Funds are best effort: a failed read does not undo the sign-in.
    session.funds = funds(&session, urls, opts).await.ok();
    Ok(session)
}

/// `GET /user/get-funds-and-margin`: equity, plus commodity if Upstox reports it.
pub async fn funds(s: &BrokerSession, urls: &BrokerUrls, opts: &KiteOptions) -> Result<Funds> {
    let http = client(opts)?;
    let api = urls.upstox_api.trim_end_matches('/');
    let res = http
        .get(format!("{api}/user/get-funds-and-margin"))
        .header("accept", "application/json")
        .header("Authorization", format!("Bearer {}", s.access_token))
        .send()
        .await
        .map_err(|e| net("Upstox funds", e))?;
    let status = res.status().as_u16();
    let body = parse_json(&res.text().await.unwrap_or_default());
    if status >= 400 || body.get("status").and_then(Value::as_str) == Some("error") {
        return Err(refused(status, format!("Upstox funds: {}", upstox_error(&body, status))));
    }
    let segs: Vec<&Value> = ["equity", "commodity"].iter().filter_map(|k| body["data"].get(*k)).filter(|v| v.is_object()).collect();
    let sum = |key: &str| segs.iter().map(|v| num_at(v, key)).sum::<f64>();
    let lines: [(&str, &str); 5] = [
        ("Pay-in today", "payin_amount"),
        ("Adhoc margin", "adhoc_margin"),
        ("Notional cash", "notional_cash"),
        ("SPAN", "span_margin"),
        ("Exposure", "exposure_margin"),
    ];
    let details = lines.iter().map(|(l, k)| (l.to_string(), sum(k))).filter(|(_, v)| *v != 0.0).collect();
    Ok(Funds::new(sum("available_margin"), sum("used_margin"), 0.0, details))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_read_from_the_redirect() {
        assert_eq!(code_of("http://127.0.0.1:3001/cb?code=ABC123&state=x"), "ABC123");
        assert_eq!(code_of("http://127.0.0.1:3001/cb"), "");
        assert_eq!(code_of(""), "");
    }

    #[test]
    fn request_ids_differ() {
        assert_ne!(request_id(), request_id());
        assert!(request_id().starts_with("WPRO-"));
    }

    #[test]
    fn pin_is_standard_base64() {
        assert_eq!(base64::engine::general_purpose::STANDARD.encode(b"1234"), "MTIzNA==");
    }

    #[test]
    fn the_misleading_code_is_explained() {
        let body = json!({"errors": [{"errorCode": "UDAPI100069", "message": "Invalid credentials"}]});
        let m = upstox_error(&body, 401);
        assert!(m.contains("redirect URL matches"), "{m}");
    }
}
