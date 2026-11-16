//! kite-engine: runs the REST API in front of a broker.
//!
//! Default is PAPER mode (no real orders). Live trading needs KITE_LIVE=1 plus
//! KITE_API_KEY and KITE_ACCESS_TOKEN, so it can never start by accident.

use kite_engine::accounts::Accounts;
use kite_engine::login::BrokerUrls;
use kite_engine::api::{router, AppState};
use kite_engine::leg::run_reconciler;
use kite_engine::ws::{run_order_stream, WsConfig};
use kite_engine::{Broker, Engine, KiteClient, KiteOptions, LegManager, ManagerConfig, PaperBroker, RiskLimits};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("{msg}");
    std::process::exit(2)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let Some(token) = env("ENGINE_TOKEN") else {
        die("ENGINE_TOKEN is required: the API token your app sends as 'Authorization: Bearer ...'");
    };
    if token.len() < 16 {
        die("ENGINE_TOKEN must be at least 16 characters");
    }

    let live = env("KITE_LIVE").as_deref() == Some("1");
    let mut paper: Option<Arc<PaperBroker>> = None;
    let mut ws_cfg: Option<WsConfig> = None;
    let broker: Arc<dyn Broker> = if live {
        let (Some(key), Some(access)) = (env("KITE_API_KEY"), env("KITE_ACCESS_TOKEN")) else {
            die("LIVE mode needs KITE_API_KEY and KITE_ACCESS_TOKEN");
        };
        let local_ip = env("KITE_LOCAL_IP")
            .map(|s| s.parse::<IpAddr>().unwrap_or_else(|_| die("KITE_LOCAL_IP is not a valid IP address")));
        let opts = KiteOptions { base_url: None, local_ip, proxy: env("KITE_PROXY") };
        ws_cfg = Some(WsConfig::kite(&key, &access));
        let client = KiteClient::with_options(key, access, opts).unwrap_or_else(|e| die(format!("cannot start the Kite client: {e}")));
        tracing::warn!("LIVE MODE: orders go to Zerodha and use real money");
        Arc::new(client)
    } else {
        tracing::info!("PAPER mode: no real orders are sent");
        let p = Arc::new(PaperBroker::new());
        paper = Some(p.clone());
        p
    };

    let mut risk = RiskLimits::default();
    if let Some(q) = env("MAX_ORDER_QTY").and_then(|s| s.parse().ok()) {
        risk.max_quantity = q;
    }
    let engine = Arc::new(Engine::new(broker, risk));
    let legs = LegManager::new(engine.clone(), ManagerConfig::default());

    let postback_secret = env("KITE_API_SECRET").map(Arc::new);
    if postback_secret.is_some() {
        tracing::info!("postbacks enabled at POST /postback/zerodha (checksum verified)");
    }

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(run_reconciler(legs.clone(), Duration::from_secs(2), stop_rx.clone()));
    if let Some(cfg) = ws_cfg {
        tokio::spawn(run_order_stream(cfg, legs.clone(), stop_rx.clone()));
    }

    // Accounts: secrets are sealed with ACCOUNTS_KEY (or, if unset, the engine
    // token). Lose the key and the file cannot be opened; the accounts must be re-entered.
    let master = env("ACCOUNTS_KEY").unwrap_or_else(|| token.clone());
    let file = std::path::PathBuf::from(env("ACCOUNTS_FILE").unwrap_or_else(|| "accounts.enc".into()));
    let account_opts = KiteOptions {
        base_url: None,
        local_ip: env("KITE_LOCAL_IP").and_then(|s| s.parse::<IpAddr>().ok()),
        proxy: env("KITE_PROXY"),
    };
    let accounts = Accounts::open(&master, Some(file.clone()), BrokerUrls::default(), account_opts)
        .unwrap_or_else(|e| die(format!("cannot open {}: {e}", file.display())));
    tracing::info!("accounts file: {}", file.display());

    let state = AppState { engine, legs, token: Arc::new(token), postback_secret, paper, accounts };
    let bind: SocketAddr = env("ENGINE_BIND")
        .unwrap_or_else(|| "127.0.0.1:8088".into())
        .parse()
        .unwrap_or_else(|_| die("ENGINE_BIND is not a valid address like 127.0.0.1:8088"));
    let listener = tokio::net::TcpListener::bind(bind).await.unwrap_or_else(|e| die(format!("cannot listen on {bind}: {e}")));
    tracing::info!("listening on http://{bind}");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = stop_tx.send(true);
        })
        .await
        .expect("server error");
}
