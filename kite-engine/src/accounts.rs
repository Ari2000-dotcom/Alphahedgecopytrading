//! The accounts store: every broker account, its secrets sealed on disk, and
//! the signed-in session each one has today.
//!
//! - Secrets (passwords, API secrets, TOTP secrets) are write-only: the API
//!   reports only *which* are saved, never their values.
//! - Access tokens live in memory only. They expire every morning, so they are
//!   never worth writing down; the engine signs in again instead.
//! - Nothing signs in by itself. A sign-in happens only when the user asks:
//!   one account, or "Login all" for every account marked auto-login. A
//!   background retry of a wrong password could lock a broker account.

use crate::crypto::Vault;
use crate::error::{Error, Result};
use crate::kite::{KiteClient, KiteOptions};
use crate::login::{BrokerSession, BrokerUrls, Creds, Funds};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub alias: String,
    pub broker: String,
    pub user_id: String,
    pub auto_login: bool,
    /// Sign-in details that are not secret (mobile, redirect URL).
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    /// Everything else the desktop app keeps per account (risk, sizing...).
    #[serde(default)]
    pub settings: Value,
}

#[derive(Debug, Clone, Default)]
struct Session {
    tokens: Option<BrokerSession>,
    logged_in_at: Option<u64>,
    error: Option<String>,
    funds: Option<Funds>,
    /// Why the last margin read failed, when it did.
    funds_error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Upsert {
    pub alias: String,
    pub broker: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub auto_login: bool,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    /// Only the secrets being set or replaced. Missing or blank = keep what is saved.
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    #[serde(default)]
    pub settings: Value,
}

pub struct Accounts {
    vault: Vault,
    path: Option<PathBuf>,
    records: Mutex<Vec<Record>>,
    sessions: Mutex<BTreeMap<String, Session>>,
    urls: BrokerUrls,
    opts: KiteOptions,
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// When the broker last expired every token. A session older than this is
/// dead. Kite resets at 06:00 IST (00:30 UTC) and Upstox at about 03:30 IST
/// (22:00 UTC). Angel, Kotak and Nubra do not publish a time, so they use the
/// earlier one: a fresh sign-in each morning is harmless.
pub fn last_token_reset(broker: &str, now: u64) -> u64 {
    let offset: u64 = if broker == "zerodha" { 30 * 60 } else { 22 * 3600 };
    now.saturating_sub(offset) / 86_400 * 86_400 + offset
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Accounts {
    /// `path = None` keeps everything in memory (tests).
    pub fn open(master: &str, path: Option<PathBuf>, urls: BrokerUrls, opts: KiteOptions) -> Result<Arc<Self>> {
        let vault = Vault::new(master);
        let records: Vec<Record> = match &path {
            Some(p) if p.exists() => {
                let sealed = std::fs::read(p).map_err(|e| Error::BadResponse(format!("cannot read {}: {e}", p.display())))?;
                serde_json::from_slice(&vault.open(&sealed)?).map_err(|e| Error::BadResponse(e.to_string()))?
            }
            _ => Vec::new(),
        };
        Ok(Arc::new(Self { vault, path, records: Mutex::new(records), sessions: Mutex::default(), urls, opts }))
    }

    fn persist(&self, records: &[Record]) -> Result<()> {
        let Some(p) = &self.path else { return Ok(()) };
        let sealed = self.vault.seal(&serde_json::to_vec(records).map_err(|e| Error::BadResponse(e.to_string()))?);
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, sealed)
            .and_then(|_| std::fs::rename(&tmp, p))
            .map_err(|e| Error::BadResponse(format!("cannot write {}: {e}", p.display())))
    }

    fn view(&self, r: &Record) -> Value {
        let s = self.sessions.lock().unwrap().get(&r.id).cloned().unwrap_or_default();
        let live = s.tokens.is_some() && s.logged_in_at.is_some_and(|t| t >= last_token_reset(&r.broker, now_secs()));
        let status = if r.broker == "paper" {
            "sim"
        } else if live {
            "in"
        } else {
            "need"
        };
        json!({
            "id": r.id, "alias": r.alias, "broker": r.broker, "user_id": r.user_id,
            "auto_login": r.auto_login, "fields": r.fields, "settings": r.settings,
            // Which secrets exist, never what they are.
            "saved": r.secrets.iter().filter(|(_, v)| !v.is_empty()).map(|(k, _)| (k.clone(), true)).collect::<BTreeMap<_, _>>(),
            "session": {
                "status": status, "error": s.error, "logged_in_at": s.logged_in_at,
                "available": s.funds.as_ref().map(|f| f.available), "funds": s.funds, "funds_error": s.funds_error,
            },
        })
    }

    pub fn list(&self) -> Vec<Value> {
        self.records.lock().unwrap().iter().map(|r| self.view(r)).collect()
    }

    pub fn get(&self, id: &str) -> Option<Value> {
        self.records.lock().unwrap().iter().find(|r| r.id == id).map(|r| self.view(r))
    }

    pub fn upsert(&self, id: &str, u: Upsert) -> Result<Value> {
        if !valid_id(id) {
            return Err(Error::Validation("account id must be letters, digits, - or _".into()));
        }
        if u.alias.trim().is_empty() {
            return Err(Error::Validation("alias is required".into()));
        }
        if !["zerodha", "angel", "kotak", "upstox", "nubra", "paper"].contains(&u.broker.as_str()) {
            return Err(Error::Validation(format!("unknown broker {}", u.broker)));
        }
        let mut recs = self.records.lock().unwrap();
        let old = recs.iter().position(|r| r.id == id);
        let mut secrets = old.map(|i| recs[i].secrets.clone()).unwrap_or_default();
        // A different broker's saved secrets mean nothing here.
        if old.is_some_and(|i| recs[i].broker != u.broker) {
            secrets.clear();
        }
        for (k, v) in u.secrets {
            if !v.trim().is_empty() {
                secrets.insert(k, v.trim().to_string());
            }
        }
        let rec = Record {
            id: id.to_string(),
            alias: u.alias.trim().to_string(),
            broker: u.broker,
            user_id: u.user_id.trim().to_string(),
            auto_login: u.auto_login,
            fields: u.fields,
            secrets,
            settings: u.settings,
        };
        let credentials_changed = old.map_or(true, |i| {
            let o = &recs[i];
            o.secrets != rec.secrets || o.user_id != rec.user_id || o.broker != rec.broker
        });
        match old {
            Some(i) => recs[i] = rec,
            None => recs.push(rec),
        }
        self.persist(&recs)?;
        let out = self.view(recs.iter().find(|r| r.id == id).unwrap());
        drop(recs);
        if credentials_changed {
            // New details deserve a fresh start: forget the old session and the failure count.
            self.sessions.lock().unwrap().remove(id);
        }
        Ok(out)
    }

    /// One account's saved secrets, for the desk to show when the user asks to
    /// see them. Nothing else ever returns these: `list` and `get` only say
    /// which are saved. Every route is behind the engine token, which already
    /// allows signing in and trading on the account.
    pub fn secrets(&self, id: &str) -> Result<BTreeMap<String, String>> {
        let recs = self.records.lock().unwrap();
        let r = recs.iter().find(|r| r.id == id).ok_or_else(|| Error::NotFound(id.into()))?;
        tracing::info!(account = %id, "saved sign-in details were read back for display");
        Ok(r.secrets.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (k.clone(), v.clone())).collect())
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut recs = self.records.lock().unwrap();
        let n = recs.len();
        recs.retain(|r| r.id != id);
        if recs.len() == n {
            return Err(Error::NotFound(id.into()));
        }
        self.persist(&recs)?;
        drop(recs);
        self.sessions.lock().unwrap().remove(id);
        Ok(())
    }

    /// Signs one account in now.
    pub async fn login(&self, id: &str) -> Result<Value> {
        let rec = self
            .records
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| Error::NotFound(id.into()))?;
        let outcome = self.sign_in(&rec).await;
        {
            let mut all = self.sessions.lock().unwrap();
            let s = all.entry(id.into()).or_default();
            match &outcome {
                Ok(sess) => {
                    *s = Session {
                        funds_error: sess.funds.is_none().then(|| "Signed in, but the margin could not be read. Use Refresh margins to try again".to_string()),
                        funds: sess.funds.clone(),
                        tokens: Some(sess.clone()),
                        logged_in_at: Some(now_secs()),
                        error: None,
                    };
                }
                Err(e) => {
                    s.tokens = None;
                    s.error = Some(e.to_string());
                }
            }
        }
        let view = self.get(id).ok_or_else(|| Error::NotFound(id.into()))?;
        outcome.map(|_| view)
    }

    async fn sign_in(&self, r: &Record) -> Result<BrokerSession> {
        if r.broker == "paper" {
            return Err(Error::Validation("the simulator needs no sign-in".into()));
        }
        let c = Creds { user_id: &r.user_id, secrets: &r.secrets, fields: &r.fields };
        crate::login::login(&r.broker, &c, &self.urls, &self.opts).await
    }

    /// Signs in every account marked auto-login, all at once. One account
    /// failing does not stop the others: each gets its own result.
    pub async fn login_all(&self) -> Vec<Value> {
        let ids: Vec<String> = self
            .records
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.auto_login && r.broker != "paper")
            .map(|r| r.id.clone())
            .collect();
        futures_util::future::join_all(ids.iter().map(|id| async move {
            match self.login(id).await {
                Ok(v) => json!({"id": id, "ok": true, "account": v}),
                Err(e) => json!({"id": id, "ok": false, "error": e.to_string(), "account": self.get(id)}),
            }
        }))
        .await
    }

    /// Reads one signed-in account's margin from its broker again.
    pub async fn refresh_funds(&self, id: &str) -> Result<Value> {
        let rec = self.records.lock().unwrap().iter().find(|r| r.id == id).cloned().ok_or_else(|| Error::NotFound(id.into()))?;
        let tokens = self.sessions.lock().unwrap().get(id).and_then(|s| s.tokens.clone());
        let Some(tokens) = tokens else {
            return Err(Error::Validation("sign this account in first".into()));
        };
        let c = Creds { user_id: &rec.user_id, secrets: &rec.secrets, fields: &rec.fields };
        let read = crate::login::funds(&rec.broker, &c, &tokens, &self.urls, &self.opts).await;
        {
            let mut all = self.sessions.lock().unwrap();
            if let Some(s) = all.get_mut(id) {
                match &read {
                    Ok(f) => {
                        s.funds = Some(f.clone());
                        s.funds_error = None;
                    }
                    // Keep the last good numbers on screen and say why they are stale.
                    Err(e) => s.funds_error = Some(e.to_string()),
                }
            }
        }
        self.get(id).ok_or_else(|| Error::NotFound(id.into()))
    }

    /// Reads every signed-in account's margin at once. Accounts that are not
    /// signed in are left out; one broker failing does not affect the others.
    pub async fn refresh_funds_all(&self) -> Vec<Value> {
        let ids: Vec<String> = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, s)| s.tokens.is_some())
            .map(|(id, _)| id.clone())
            .collect();
        futures_util::future::join_all(ids.iter().map(|id| async move { self.refresh_funds(id).await.ok() }))
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// The signed-in Kite client for an account, if it has a live session.
    pub fn kite_client(&self, id: &str) -> Option<KiteClient> {
        let (key, token) = {
            let recs = self.records.lock().unwrap();
            let r = recs.iter().find(|r| r.id == id && r.broker == "zerodha")?;
            let token = self.sessions.lock().unwrap().get(id)?.tokens.as_ref()?.access_token.clone();
            (r.secrets.get("apiKey")?.clone(), token)
        };
        let opts = KiteOptions { base_url: Some(self.urls.zerodha_api.clone()), ..self.opts.clone() };
        KiteClient::with_options(key, token, opts).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_reset_is_0600_ist() {
        // 2026-01-02 00:00:00 UTC. Before 00:30 UTC the last reset was the day before.
        let day = 1_767_312_000;
        assert_eq!(last_token_reset("zerodha", day + 30 * 60 - 1), day - 86_400 + 30 * 60);
        assert_eq!(last_token_reset("zerodha", day + 30 * 60), day + 30 * 60);
        assert_eq!(last_token_reset("zerodha", day + 20 * 3600), day + 30 * 60);
        // Upstox and the rest: 22:00 UTC (03:30 IST).
        assert_eq!(last_token_reset("upstox", day + 21 * 3600), day - 2 * 3600);
        assert_eq!(last_token_reset("angel", day + 22 * 3600), day + 22 * 3600);
    }
}
