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

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![engine_health])
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
}
