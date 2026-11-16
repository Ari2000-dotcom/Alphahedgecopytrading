// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::time::{Duration, Instant};

#[derive(Serialize)]
struct Health {
    broker: String,
    latency_ms: u64,
}

/// Only these addresses may be called: your own machine over plain http, and
/// anything else only over https. The page cannot point the host at an
/// arbitrary http address.
fn allowed(url: &str) -> bool {
    let local = ["http://127.0.0.1", "http://localhost", "http://[::1]"];
    url.starts_with("https://")
        || local.iter().any(|p| {
            url.strip_prefix(p).is_some_and(|rest| rest.is_empty() || rest.starts_with(':') || rest.starts_with('/'))
        })
}

/// Ask the engine if it is alive and which broker it runs. Needs no token.
#[tauri::command]
async fn engine_health(url: String) -> Result<Health, String> {
    if !allowed(&url) {
        return Err("engine address must be https, or http on this machine".into());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let started = Instant::now();
    let resp = client
        .get(format!("{}/health", url.trim_end_matches('/')))
        .send()
        .await
        .map_err(|_| "engine not reachable".to_string())?;
    let latency_ms = started.elapsed().as_millis() as u64;
    let body: serde_json::Value = resp.json().await.map_err(|_| "unexpected reply".to_string())?;
    Ok(Health { broker: body["broker"].as_str().unwrap_or("unknown").to_string(), latency_ms })
}

/// The engine token: ENGINE_TOKEN in the environment, or the first line of
/// %APPDATA%\AlphaHedge Desk\engine-token.txt. It stays in this host process;
/// the web page never sees it.
fn engine_token() -> Result<String, String> {
    if let Ok(t) = std::env::var("ENGINE_TOKEN") {
        if !t.trim().is_empty() {
            return Ok(t.trim().to_string());
        }
    }
    let file = std::env::var("APPDATA").map(|d| std::path::PathBuf::from(d).join("AlphaHedge Desk").join("engine-token.txt"));
    if let Ok(f) = &file {
        if let Ok(t) = std::fs::read_to_string(f) {
            if let Some(line) = t.lines().next().filter(|l| !l.trim().is_empty()) {
                return Ok(line.trim().to_string());
            }
        }
    }
    Err(format!(
        "no engine token: set ENGINE_TOKEN, or save it as the first line of {}",
        file.map(|f| f.display().to_string()).unwrap_or_else(|_| r"engine-token.txt in %APPDATA%\AlphaHedge Desk".into())
    ))
}

/// Only the accounts routes, and only plain ids in the path.
fn accounts_path_ok(path: &str) -> bool {
    path.strip_prefix("/accounts").is_some_and(|rest| {
        rest.is_empty()
            || (rest.starts_with('/')
                && rest[1..].split('/').count() <= 2
                && rest[1..].split('/').all(|seg| {
                    !seg.is_empty() && seg.len() <= 40 && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                }))
    })
}

/// Calls the engine's accounts routes with the token and returns its JSON. An
/// engine error comes back as the engine's own message.
#[tauri::command]
async fn accounts_call(url: String, method: String, path: String, body: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    if !allowed(&url) {
        return Err("engine address must be https, or http on this machine".into());
    }
    if !accounts_path_ok(&path) {
        return Err("not an accounts route".into());
    }
    let m = match method.as_str() {
        "GET" => reqwest::Method::GET,
        "PUT" => reqwest::Method::PUT,
        "POST" => reqwest::Method::POST,
        "DELETE" => reqwest::Method::DELETE,
        _ => return Err("unsupported method".into()),
    };
    let token = engine_token()?;
    // A sign-in walks through several pages of the broker, so allow it time.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let mut rb = client.request(m, format!("{}{}", url.trim_end_matches('/'), path)).bearer_auth(token);
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    let resp = rb.send().await.map_err(|_| "engine not reachable".to_string())?;
    let status = resp.status();
    let v: serde_json::Value = resp.json().await.map_err(|_| format!("unexpected reply (HTTP {status})"))?;
    if status.is_success() {
        Ok(v)
    } else if status == reqwest::StatusCode::UNAUTHORIZED {
        Err("the engine refused the token: it does not match the engine's ENGINE_TOKEN".into())
    } else {
        Err(v["message"].as_str().or(v["error"].as_str()).unwrap_or("the engine refused the request").to_string())
    }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![engine_health, accounts_call])
        .run(tauri::generate_context!())
        .expect("error while running the app");
}

#[cfg(test)]
mod tests {
    use super::allowed;

    #[test]
    fn only_https_or_local_http() {
        assert!(allowed("https://engine.example.com"));
        assert!(allowed("http://127.0.0.1:8088"));
        assert!(allowed("http://localhost:8088"));
        assert!(!allowed("http://example.com"));
        assert!(!allowed("http://127.0.0.1.evil.com"));
        assert!(!allowed("http://localhost.evil.com"));
        assert!(!allowed("ftp://x"));
    }

    #[test]
    fn only_account_routes() {
        use super::accounts_path_ok as ok;
        assert!(ok("/accounts") && ok("/accounts/a1") && ok("/accounts/a1/login") && ok("/accounts/login-all"));
        assert!(!ok("/accounts/a1/login/x") && !ok("/accounts/../orders") && !ok("/accountsx"));
        assert!(!ok("/orders") && !ok("/accounts/a1?x=1") && !ok("/accounts//login"));
    }
}
