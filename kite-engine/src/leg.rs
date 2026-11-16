//! Entry order plus a stop-loss placed from the entry's real fill price.
//!
//! One *leg* = one entry order and the SL that protects it. The flow:
//!
//! ```text
//! place entry (tag = leg id) -> fill seen (socket / postback / poll)
//!   -> SL trigger = fill price +/- percent -> place SL for the filled quantity
//!   -> more fills: modify the SL quantity (never above what is filled)
//!   -> SL fills: leg closed.   SL cannot be placed: alert, optionally exit.
//! ```
//!
//! Three sources report fills and all feed `LegManager::on_update`. It is safe
//! to call with the same update many times: each leg is handled under its own
//! lock and a fill is only acted on when it is more than already seen.

use crate::engine::Engine;
use crate::error::{Error, Result};
use crate::types::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Mutex as AsyncMutex};

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

// ---------------------------------------------------------------- settings

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlKind {
    #[serde(rename = "SL")]
    Sl,
    #[serde(rename = "SL-M")]
    SlM,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnSlFailure {
    /// Close the filled quantity at market, and alert.
    #[default]
    ExitAtMarket,
    /// Only alert. The leg stays open and unprotected until you act.
    AlertOnly,
}

fn d_kind() -> SlKind {
    SlKind::Sl
}
fn d_buffer() -> f64 {
    5.0
}
fn d_tick() -> f64 {
    0.05
}

/// How the SL is set. `percent` is of the entry's fill price (the premium):
/// 50 means a short filled at 100 gets a buy SL at 150, and a long filled at
/// 100 gets a sell SL at 50.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlSpec {
    pub percent: f64,
    #[serde(default = "d_kind")]
    pub kind: SlKind,
    /// For `SL` (stop-limit): the limit price sits this percent beyond the
    /// trigger so the order can still fill in a fast market.
    #[serde(default = "d_buffer")]
    pub limit_buffer_pct: f64,
    #[serde(default = "d_tick")]
    pub tick: f64,
    #[serde(default)]
    pub on_failure: OnSlFailure,
}

impl SlSpec {
    pub fn validate(&self, entry_side: Side) -> Result<()> {
        let bad = |m: &str| Err(Error::Validation(m.to_string()));
        if !(self.percent.is_finite() && self.percent > 0.0) {
            return bad("sl.percent must be above zero");
        }
        if entry_side == Side::Buy && self.percent >= 100.0 {
            return bad("sl.percent must be below 100 for a bought leg");
        }
        if !(self.tick.is_finite() && self.tick > 0.0) {
            return bad("sl.tick must be above zero");
        }
        if !(self.limit_buffer_pct.is_finite() && (0.0..=50.0).contains(&self.limit_buffer_pct)) {
            return bad("sl.limit_buffer_pct must be between 0 and 50");
        }
        Ok(())
    }

    /// Trigger and (for stop-limit) limit price for a fill at `avg`.
    /// A short's SL is a buy above the fill, rounded up; a long's is a sell
    /// below it, rounded down.
    pub fn levels(&self, entry_side: Side, avg: f64) -> Result<(f64, Option<f64>)> {
        let up = entry_side == Side::Sell;
        let sign = if up { 1.0 } else { -1.0 };
        let trigger = round_to_tick(avg * (1.0 + sign * self.percent / 100.0), self.tick, up);
        if trigger < self.tick {
            return Err(Error::Validation(format!("SL trigger {trigger} is below one tick")));
        }
        let price = match self.kind {
            SlKind::SlM => None,
            SlKind::Sl => {
                let p = round_to_tick(trigger * (1.0 + sign * self.limit_buffer_pct / 100.0), self.tick, up);
                if p < self.tick {
                    return Err(Error::Validation(format!("SL limit price {p} is below one tick")));
                }
                Some(p)
            }
        };
        Ok((trigger, price))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LegRequest {
    /// Up to 18 letters and digits. Generated if absent.
    #[serde(default)]
    pub leg_id: Option<String>,
    /// The entry order. Leave `tag` empty: the engine sets it to the leg id.
    pub entry: OrderRequest,
    pub sl: SlSpec,
}

#[derive(Debug, Clone)]
pub struct ManagerConfig {
    /// Waits between attempts to place or modify the SL after a network-type failure.
    pub retry_delays: Vec<Duration>,
    /// After placing the entry, also read it back after each of these delays,
    /// in case the push message is late.
    pub check_delays: Vec<Duration>,
}

impl Default for ManagerConfig {
    fn default() -> Self {
        let ms = Duration::from_millis;
        Self {
            retry_delays: vec![ms(200), ms(500), ms(1000), ms(2000)],
            check_delays: vec![ms(150), ms(400), ms(1000)],
        }
    }
}

// ------------------------------------------------------------------- state

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", content = "reason", rename_all = "snake_case")]
pub enum LegState {
    /// Entry sent, nothing filled yet.
    PendingEntry,
    /// Something is filled and an SL covers it.
    Protected,
    /// Filled quantity with no working SL. Needs a person (an alert was sent).
    Unprotected(String),
    Closed(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct LegEvent {
    pub at_ms: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Leg {
    pub id: String,
    pub entry: OrderRequest,
    pub sl: SlSpec,
    pub state: LegState,
    pub entry_order_id: Option<String>,
    pub sl_order_id: Option<String>,
    pub exit_order_id: Option<String>,
    pub filled_qty: u32,
    pub avg_price: f64,
    pub entry_complete: bool,
    pub sl_qty: u32,
    pub sl_trigger: Option<f64>,
    pub sl_price: Option<f64>,
    pub exit_price: Option<f64>,
    /// When the engine first saw a fill, and when the SL was accepted. The gap
    /// is the time the leg had no SL.
    pub fill_seen_ms: Option<u64>,
    pub sl_acked_ms: Option<u64>,
    #[serde(skip)]
    exiting: bool,
    pub events: Vec<LegEvent>,
}

impl Leg {
    fn event(&mut self, msg: impl Into<String>) {
        self.events.push(LegEvent { at_ms: now_ms(), message: msg.into() });
    }
    fn close(&mut self, reason: &str) {
        self.state = LegState::Closed(reason.to_string());
        self.event(format!("closed: {reason}"));
    }
    fn is_closed(&self) -> bool {
        matches!(self.state, LegState::Closed(_))
    }
    pub fn sl_tag(&self) -> String {
        format!("{}S", self.id)
    }
    pub fn exit_tag(&self) -> String {
        format!("{}X", self.id)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Alert {
    pub leg_id: String,
    pub message: String,
}

#[derive(Clone, Copy)]
enum Role {
    Entry,
    Sl,
    Exit,
}

// ----------------------------------------------------------------- manager

pub struct LegManager {
    engine: Arc<Engine>,
    cfg: ManagerConfig,
    legs: Mutex<HashMap<String, Arc<AsyncMutex<Leg>>>>,
    by_order: Mutex<HashMap<String, (String, Role)>>,
    by_tag: Mutex<HashMap<String, (String, Role)>>,
    alerts: broadcast::Sender<Alert>,
    counter: Mutex<u64>,
}

impl LegManager {
    pub fn new(engine: Arc<Engine>, cfg: ManagerConfig) -> Arc<Self> {
        let (alerts, _) = broadcast::channel(64);
        Arc::new(Self {
            engine,
            cfg,
            legs: Mutex::default(),
            by_order: Mutex::default(),
            by_tag: Mutex::default(),
            alerts,
            counter: Mutex::new(0),
        })
    }

    /// Alerts (SL could not be placed, SL cancelled outside the engine, ...).
    pub fn subscribe_alerts(&self) -> broadcast::Receiver<Alert> {
        self.alerts.subscribe()
    }

    fn alert(&self, leg: &mut Leg, msg: String) {
        tracing::error!(leg = %leg.id, "{msg}");
        leg.event(format!("ALERT: {msg}"));
        let _ = self.alerts.send(Alert { leg_id: leg.id.clone(), message: msg });
    }

    fn register_order(&self, order_id: &str, leg_id: &str, role: Role) {
        self.by_order.lock().unwrap().insert(order_id.to_string(), (leg_id.to_string(), role));
    }

    fn new_id(&self) -> String {
        let mut c = self.counter.lock().unwrap();
        *c += 1;
        // 'L' + ms clock in base 36 + counter: short, unique, letters and digits only.
        format!("L{}{}", to_base36(now_ms()), to_base36(*c))
    }

    // -------------------------------------------------------------- create

    /// Place the entry order. The SL follows when the fill is seen.
    /// Returns (leg id, entry order id).
    pub async fn create_leg(self: &Arc<Self>, req: LegRequest) -> Result<(String, String)> {
        let mut entry = req.entry.clone();
        if entry.tag.is_some() {
            return Err(Error::Validation("leave entry.tag empty: the engine sets it to the leg id".into()));
        }
        req.sl.validate(entry.side)?;
        let id = match req.leg_id {
            Some(id) => {
                if id.is_empty() || id.len() > 18 || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Err(Error::Validation("leg_id must be 1 to 18 letters and digits".into()));
                }
                id
            }
            None => self.new_id(),
        };
        entry.tag = Some(id.clone());
        entry.validate()?;

        let leg = Leg {
            id: id.clone(),
            entry: entry.clone(),
            sl: req.sl,
            state: LegState::PendingEntry,
            entry_order_id: None,
            sl_order_id: None,
            exit_order_id: None,
            filled_qty: 0,
            avg_price: 0.0,
            entry_complete: false,
            sl_qty: 0,
            sl_trigger: None,
            sl_price: None,
            exit_price: None,
            fill_seen_ms: None,
            sl_acked_ms: None,
            exiting: false,
            events: vec![],
        };
        let cell = Arc::new(AsyncMutex::new(leg));
        // Hold the leg's lock from before the order is sent until its id is
        // recorded. A fill that arrives in between finds the leg by tag and
        // waits here, so it is never lost.
        let mut guard = cell.clone().lock_owned().await;
        {
            let mut legs = self.legs.lock().unwrap();
            if legs.contains_key(&id) {
                return Err(Error::Duplicate(id));
            }
            legs.insert(id.clone(), cell);
        }
        {
            let mut tags = self.by_tag.lock().unwrap();
            tags.insert(id.clone(), (id.clone(), Role::Entry));
            tags.insert(guard.sl_tag(), (id.clone(), Role::Sl));
            tags.insert(guard.exit_tag(), (id.clone(), Role::Exit));
        }

        match self.engine.place(&entry).await {
            Ok(order_id) => {
                guard.entry_order_id = Some(order_id.clone());
                guard.event(format!("entry placed: order {order_id}"));
                self.register_order(&order_id, &id, Role::Entry);
                drop(guard);
                self.schedule_checks(order_id.clone());
                Ok((id, order_id))
            }
            Err(e @ Error::Unknown(_)) => {
                // It may exist at the broker. Keep the leg: the reconciler
                // finds the order by its tag.
                guard.event(format!("entry outcome unknown: {e}"));
                Err(e)
            }
            Err(e) => {
                drop(guard);
                self.forget_leg(&id);
                Err(e)
            }
        }
    }

    fn forget_leg(&self, id: &str) {
        self.legs.lock().unwrap().remove(id);
        let mut tags = self.by_tag.lock().unwrap();
        tags.remove(id);
        tags.remove(&format!("{id}S"));
        tags.remove(&format!("{id}X"));
    }

    fn cell(&self, id: &str) -> Option<Arc<AsyncMutex<Leg>>> {
        self.legs.lock().unwrap().get(id).cloned()
    }

    /// Read the entry back shortly after placing it, in case the push is late.
    fn schedule_checks(self: &Arc<Self>, order_id: String) {
        if self.cfg.check_delays.is_empty() {
            return;
        }
        let me = self.clone();
        tokio::spawn(async move {
            let mut last = Duration::ZERO;
            for d in me.cfg.check_delays.clone() {
                tokio::time::sleep(d.saturating_sub(last)).await;
                last = d;
                if let Some(row) = me.fetch_row(&order_id).await {
                    me.on_update(&row).await;
                }
            }
        });
    }

    // ---------------------------------------------------------- read side

    pub async fn get(&self, id: &str) -> Option<Leg> {
        let cell = self.cell(id)?;
        let leg = cell.lock().await.clone();
        Some(leg)
    }

    pub async fn list(&self) -> Vec<Leg> {
        let cells: Vec<_> = self.legs.lock().unwrap().values().cloned().collect();
        let mut out = vec![];
        for c in cells {
            out.push(c.lock().await.clone());
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// True while any leg still needs watching.
    pub async fn has_active(&self) -> bool {
        self.list().await.iter().any(|l| !l.is_closed())
    }

    async fn fetch_row(&self, order_id: &str) -> Option<Order> {
        self.engine.order_history(order_id).await.ok()?.into_iter().last()
    }

    /// Look the order up by tag. `Err` means we could not tell.
    async fn find_by_tag(&self, tag: &str) -> Result<Option<Order>> {
        Ok(self.engine.orders().await?.into_iter().rev().find(|o| o.tag.as_deref() == Some(tag)))
    }

    // ------------------------------------------------------- update intake

    /// Feed one order update (from the socket, a postback, or a poll).
    pub async fn on_update(&self, u: &Order) {
        let found = self
            .by_order
            .lock()
            .unwrap()
            .get(&u.order_id)
            .cloned()
            .or_else(|| u.tag.as_ref().and_then(|t| self.by_tag.lock().unwrap().get(t).cloned()));
        let Some((leg_id, role)) = found else { return };
        let Some(cell) = self.cell(&leg_id) else { return };
        let mut leg = cell.lock().await;
        self.apply(&mut leg, role, u).await;
    }

    async fn apply(&self, leg: &mut Leg, role: Role, u: &Order) {
        if leg.is_closed() {
            return;
        }
        // Learn an order id we did not get a reply for (lost reply, or a fill
        // that beat the reply).
        match role {
            Role::Entry if leg.entry_order_id.is_none() => {
                leg.entry_order_id = Some(u.order_id.clone());
                self.register_order(&u.order_id, &leg.id, Role::Entry);
            }
            Role::Sl if leg.sl_order_id.is_none() => {
                leg.sl_order_id = Some(u.order_id.clone());
                self.register_order(&u.order_id, &leg.id, Role::Sl);
            }
            Role::Exit if leg.exit_order_id.is_none() => {
                leg.exit_order_id = Some(u.order_id.clone());
                self.register_order(&u.order_id, &leg.id, Role::Exit);
            }
            _ => {}
        }
        match role {
            Role::Entry => self.on_entry_update(leg, u).await,
            Role::Sl => self.on_sl_update(leg, u).await,
            Role::Exit => self.on_exit_update(leg, u),
        }
    }

    async fn on_entry_update(&self, leg: &mut Leg, u: &Order) {
        let st = u.status.as_str();
        let ended_without_fill = matches!(st, "REJECTED" | "CANCELLED");
        if ended_without_fill && u.filled_quantity == 0 && leg.filled_qty == 0 {
            if let Some(m) = &u.status_message {
                leg.event(format!("entry {st}: {m}"));
            }
            leg.close(&format!("entry_{}", st.to_lowercase()));
            return;
        }

        let mut changed = false;
        if u.filled_quantity > leg.filled_qty {
            // Pushes can carry the fill without its average price; read it.
            let mut avg = u.average_price;
            if avg <= 0.0 {
                if let Some(id) = &leg.entry_order_id {
                    avg = self.fetch_row(id).await.map(|r| r.average_price).unwrap_or(0.0);
                }
            }
            if avg <= 0.0 {
                leg.event("fill seen but its average price is not known yet; will re-read");
                return;
            }
            leg.filled_qty = u.filled_quantity;
            leg.avg_price = avg;
            if leg.fill_seen_ms.is_none() {
                leg.fill_seen_ms = Some(now_ms());
            }
            leg.event(format!("fill: {} of {} at {avg}", leg.filled_qty, leg.entry.quantity));
            changed = true;
        }
        let ended = st == "COMPLETE" || ended_without_fill;
        if ended && !leg.entry_complete {
            leg.entry_complete = true;
            if st == "COMPLETE" && u.average_price > 0.0 && (u.average_price - leg.avg_price).abs() > 1e-9 {
                leg.avg_price = u.average_price;
            }
            changed = true;
        }
        if changed && leg.filled_qty > 0 && !matches!(leg.state, LegState::Unprotected(_)) {
            self.sync_sl(leg).await;
        }
    }

    async fn on_sl_update(&self, leg: &mut Leg, u: &Order) {
        match u.status.as_str() {
            "COMPLETE" => {
                leg.exit_price = Some(u.average_price);
                leg.close("sl_filled");
                if !leg.entry_complete {
                    // The SL is out; stop the rest of the entry from filling.
                    if let Some(id) = leg.entry_order_id.clone() {
                        let _ = self.engine.cancel(leg.entry.variety, &id).await;
                    }
                }
            }
            "REJECTED" => {
                let why = u.status_message.clone().unwrap_or_default();
                self.fail_sl(leg, format!("SL order was rejected: {why}")).await;
            }
            "CANCELLED" if !leg.exiting => {
                leg.state = LegState::Unprotected("SL cancelled outside the engine".into());
                self.alert(leg, "the SL order was cancelled outside the engine: the leg is unprotected".into());
            }
            _ => {}
        }
    }

    fn on_exit_update(&self, leg: &mut Leg, u: &Order) {
        match u.status.as_str() {
            "COMPLETE" => {
                leg.exit_price = Some(u.average_price);
                leg.close("exited");
            }
            "REJECTED" | "CANCELLED" => {
                let why = u.status_message.clone().unwrap_or_default();
                leg.state = LegState::Unprotected("exit order failed".into());
                self.alert(leg, format!("the exit order was {}: {why}. The position is still open", u.status));
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ SL

    fn sl_request(&self, leg: &Leg, trigger: f64, price: Option<f64>) -> OrderRequest {
        OrderRequest {
            exchange: leg.entry.exchange.clone(),
            tradingsymbol: leg.entry.tradingsymbol.clone(),
            side: leg.entry.side.opposite(),
            order_type: match leg.sl.kind {
                SlKind::Sl => OrderType::Sl,
                SlKind::SlM => OrderType::SlM,
            },
            product: leg.entry.product,
            quantity: leg.filled_qty,
            price,
            trigger_price: Some(trigger),
            validity: None,
            variety: Variety::Regular,
            tag: Some(leg.sl_tag()),
            market_protection: None,
        }
    }

    /// Make the SL match what is filled: place it, or modify it.
    async fn sync_sl(&self, leg: &mut Leg) {
        if leg.is_closed() || leg.filled_qty == 0 {
            return;
        }
        let (trigger, price) = match leg.sl.levels(leg.entry.side, leg.avg_price) {
            Ok(v) => v,
            Err(e) => {
                self.fail_sl(leg, format!("cannot work out the SL: {e}")).await;
                return;
            }
        };
        match leg.sl_order_id.clone() {
            None => self.place_sl(leg, trigger, price).await,
            Some(id) => {
                if leg.sl_qty != leg.filled_qty || leg.sl_trigger != Some(trigger) {
                    self.modify_sl(leg, &id, trigger, price).await;
                }
            }
        }
    }

    async fn place_sl(&self, leg: &mut Leg, trigger: f64, price: Option<f64>) {
        let req = self.sl_request(leg, trigger, price);
        let tag = leg.sl_tag();
        let mut attempt = 0;
        loop {
            let err = match self.engine.place(&req).await {
                Ok(id) => {
                    self.sl_placed(leg, id, trigger, price);
                    return;
                }
                Err(e) => e,
            };
            match err {
                Error::Network(_) | Error::Unknown(_) | Error::Duplicate(_) => {
                    // Before trying again, find out whether the SL is already
                    // there. A second SL would be a naked position.
                    match self.find_by_tag(&tag).await {
                        Ok(Some(row)) => {
                            leg.event("SL found at the broker after an unclear reply; using it");
                            self.sl_placed(leg, row.order_id.clone(), trigger, price);
                            self.on_sl_update(leg, &row).await;
                            return;
                        }
                        Ok(None) => self.engine.forget_tag(&tag),
                        Err(_) => {}
                    }
                    match self.cfg.retry_delays.get(attempt) {
                        Some(d) => {
                            attempt += 1;
                            leg.event(format!("SL attempt {attempt} failed ({err}); retrying"));
                            tokio::time::sleep(*d).await;
                        }
                        None => {
                            self.fail_sl(leg, format!("SL could not be placed: {err}")).await;
                            return;
                        }
                    }
                }
                other => {
                    // The broker said no, or the order was invalid: retrying
                    // the same order will not change the answer.
                    self.fail_sl(leg, format!("SL could not be placed: {other}")).await;
                    return;
                }
            }
        }
    }

    fn sl_placed(&self, leg: &mut Leg, id: String, trigger: f64, price: Option<f64>) {
        self.register_order(&id, &leg.id, Role::Sl);
        leg.sl_order_id = Some(id.clone());
        leg.sl_qty = leg.filled_qty;
        leg.sl_trigger = Some(trigger);
        leg.sl_price = price;
        leg.sl_acked_ms = Some(now_ms());
        if !matches!(leg.state, LegState::Unprotected(_)) {
            leg.state = LegState::Protected;
        }
        let gap = leg.fill_seen_ms.map(|f| now_ms().saturating_sub(f));
        leg.event(format!("SL placed: order {id}, trigger {trigger}, qty {} (fill to SL ack: {gap:?} ms)", leg.sl_qty));
    }

    async fn modify_sl(&self, leg: &mut Leg, sl_id: &str, trigger: f64, price: Option<f64>) {
        let m = ModifyRequest {
            quantity: Some(leg.filled_qty),
            trigger_price: Some(trigger),
            price,
            ..Default::default()
        };
        let mut attempt = 0;
        loop {
            match self.engine.modify(Variety::Regular, sl_id, &m).await {
                Ok(_) => {
                    leg.sl_qty = leg.filled_qty;
                    leg.sl_trigger = Some(trigger);
                    leg.sl_price = price;
                    leg.event(format!("SL modified: trigger {trigger}, qty {}", leg.sl_qty));
                    return;
                }
                Err(e @ (Error::Network(_) | Error::Unknown(_))) => match self.cfg.retry_delays.get(attempt) {
                    // A modify is safe to repeat: it sets values, it does not add.
                    Some(d) => {
                        attempt += 1;
                        leg.event(format!("SL modify attempt {attempt} failed ({e}); retrying"));
                        tokio::time::sleep(*d).await;
                    }
                    None => {
                        self.fail_sl(leg, format!("SL could not be modified: {e}")).await;
                        return;
                    }
                },
                Err(e) => {
                    // Most likely the SL already triggered. Look before deciding.
                    if let Some(row) = self.fetch_row(sl_id).await {
                        if row.status == "COMPLETE" {
                            self.on_sl_update(leg, &row).await;
                            return;
                        }
                    }
                    self.fail_sl(leg, format!("SL could not be modified: {e}")).await;
                    return;
                }
            }
        }
    }

    async fn fail_sl(&self, leg: &mut Leg, reason: String) {
        leg.state = LegState::Unprotected(reason.clone());
        self.alert(leg, format!("{reason}. Filled quantity {} has no working SL", leg.filled_qty));
        if leg.sl.on_failure == OnSlFailure::ExitAtMarket {
            self.exit_at_market(leg, "emergency exit").await;
        }
    }

    // ---------------------------------------------------------------- exit

    async fn exit_at_market(&self, leg: &mut Leg, why: &str) {
        if leg.exit_order_id.is_some() || leg.filled_qty == 0 {
            return;
        }
        // Stop any unfilled part of the entry first.
        if !leg.entry_complete {
            if let Some(id) = leg.entry_order_id.clone() {
                let _ = self.engine.cancel(leg.entry.variety, &id).await;
                if let Some(row) = self.fetch_row(&id).await {
                    if row.filled_quantity > leg.filled_qty {
                        leg.filled_qty = row.filled_quantity;
                    }
                }
            }
        }
        let req = OrderRequest {
            exchange: leg.entry.exchange.clone(),
            tradingsymbol: leg.entry.tradingsymbol.clone(),
            side: leg.entry.side.opposite(),
            order_type: OrderType::Market,
            product: leg.entry.product,
            quantity: leg.filled_qty,
            price: None,
            trigger_price: None,
            validity: None,
            variety: Variety::Regular,
            tag: Some(leg.exit_tag()),
            market_protection: None,
        };
        match self.engine.place(&req).await {
            Ok(id) => {
                leg.event(format!("{why}: market order {id} for {}", leg.filled_qty));
                self.register_order(&id, &leg.id, Role::Exit);
                leg.exit_order_id = Some(id);
            }
            Err(e) => {
                self.alert(leg, format!("{why} FAILED: {e}. The position is still open"));
            }
        }
    }

    /// Close the leg now: cancel the SL, then sell/buy back at market.
    pub async fn exit_leg(&self, id: &str) -> Result<()> {
        let cell = self.cell(id).ok_or_else(|| Error::NotFound(id.to_string()))?;
        let mut leg = cell.lock().await;
        if leg.is_closed() {
            return Err(Error::Validation("the leg is already closed".into()));
        }
        if leg.filled_qty == 0 && leg.entry_complete == false {
            // Nothing filled: just stop the entry.
            if let Some(oid) = leg.entry_order_id.clone() {
                let _ = self.engine.cancel(leg.entry.variety, &oid).await;
                if let Some(row) = self.fetch_row(&oid).await {
                    self.apply(&mut leg, Role::Entry, &row).await;
                }
            }
            if !leg.is_closed() && leg.filled_qty == 0 {
                leg.close("entry_cancelled");
                return Ok(());
            }
        }
        leg.exiting = true;
        if let Some(sl_id) = leg.sl_order_id.clone() {
            let _ = self.engine.cancel(Variety::Regular, &sl_id).await;
            // If the cancel failed because the SL already filled, we are out.
            if let Some(row) = self.fetch_row(&sl_id).await {
                if row.status == "COMPLETE" {
                    self.apply(&mut leg, Role::Sl, &row).await;
                    return Ok(());
                }
            }
        }
        self.exit_at_market(&mut leg, "exit").await;
        match leg.exit_order_id {
            Some(_) => Ok(()),
            None => Err(Error::Validation("exit order could not be placed; see the leg's events".into())),
        }
    }

    /// Change the SL percent and move the SL at the broker to match.
    pub async fn set_sl_percent(&self, id: &str, percent: f64) -> Result<Leg> {
        let cell = self.cell(id).ok_or_else(|| Error::NotFound(id.to_string()))?;
        let mut leg = cell.lock().await;
        if leg.is_closed() {
            return Err(Error::Validation("the leg is already closed".into()));
        }
        let mut sl = leg.sl.clone();
        sl.percent = percent;
        sl.validate(leg.entry.side)?;
        leg.sl = sl;
        leg.event(format!("SL percent set to {percent}"));
        if leg.filled_qty > 0 {
            self.sync_sl(&mut leg).await;
        }
        Ok(leg.clone())
    }

    // ----------------------------------------------------------- reconcile

    /// Compare every open leg with the broker's order book and fix any gap:
    /// adopt orders whose reply was lost, apply missed fills, and place a
    /// missing SL. Run after every reconnect and on a timer.
    pub async fn reconcile_all(&self) {
        let cells: Vec<_> = self.legs.lock().unwrap().values().cloned().collect();
        if cells.is_empty() {
            return;
        }
        let rows = match self.engine.orders().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("reconcile: cannot read the order book: {e}");
                return;
            }
        };
        let by_id: HashMap<&str, &Order> = rows.iter().map(|o| (o.order_id.as_str(), o)).collect();
        for cell in cells {
            let mut leg = cell.lock().await;
            if leg.is_closed() {
                continue;
            }
            if leg.entry_order_id.is_none() {
                if let Some(row) = rows.iter().rev().find(|o| o.tag.as_deref() == Some(leg.id.as_str())) {
                    leg.event("entry found at the broker by its tag");
                    self.apply(&mut leg, Role::Entry, row).await;
                }
            }
            for (oid, role) in [
                (leg.entry_order_id.clone(), Role::Entry),
                (leg.sl_order_id.clone(), Role::Sl),
                (leg.exit_order_id.clone(), Role::Exit),
            ] {
                if leg.is_closed() {
                    break;
                }
                if let Some(row) = oid.as_deref().and_then(|o| by_id.get(o)) {
                    self.apply(&mut leg, role, row).await;
                }
            }
            // Filled but no SL, and not already failed: place it.
            if !leg.is_closed()
                && leg.filled_qty > 0
                && leg.sl_order_id.is_none()
                && leg.exit_order_id.is_none()
                && !matches!(leg.state, LegState::Unprotected(_))
            {
                leg.event("filled with no SL: placing it");
                self.sync_sl(&mut leg).await;
            }
        }
    }
}

/// Poll the broker on a timer while any leg is open.
pub async fn run_reconciler(mgr: Arc<LegManager>, every: Duration, mut stop: tokio::sync::watch::Receiver<bool>) {
    loop {
        tokio::select! {
            _ = stop.changed() => return,
            _ = tokio::time::sleep(every) => {}
        }
        if mgr.has_active().await {
            mgr.reconcile_all().await;
        }
    }
}

fn to_base36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = vec![];
    while n > 0 {
        out.push(D[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: SlKind) -> SlSpec {
        SlSpec { percent: 50.0, kind, limit_buffer_pct: 5.0, tick: 0.05, on_failure: OnSlFailure::AlertOnly }
    }

    #[test]
    fn short_sl_is_a_buy_above_the_fill() {
        let (t, p) = spec(SlKind::Sl).levels(Side::Sell, 100.0).unwrap();
        assert_eq!(t, 150.0);
        assert_eq!(p, Some(157.5));
    }

    #[test]
    fn long_sl_is_a_sell_below_the_fill() {
        let (t, p) = spec(SlKind::Sl).levels(Side::Buy, 100.0).unwrap();
        assert_eq!(t, 50.0);
        assert_eq!(p, Some(47.5));
    }

    #[test]
    fn rounds_to_the_tick_in_the_safe_direction() {
        // 100.03 * 1.5 = 150.045 -> a short's trigger rounds up
        let (t, _) = spec(SlKind::SlM).levels(Side::Sell, 100.03).unwrap();
        assert_eq!(t, 150.05);
        // 33.37 * 0.5 = 16.685 -> a long's trigger rounds down
        let (t, _) = spec(SlKind::SlM).levels(Side::Buy, 33.37).unwrap();
        assert_eq!(t, 16.65);
    }

    #[test]
    fn slm_has_no_limit_price_and_bad_inputs_are_refused() {
        assert_eq!(spec(SlKind::SlM).levels(Side::Sell, 100.0).unwrap().1, None);
        let mut s = spec(SlKind::Sl);
        s.percent = 100.0;
        assert!(s.validate(Side::Buy).is_err());
        assert!(s.validate(Side::Sell).is_ok());
        s.percent = 0.0;
        assert!(s.validate(Side::Sell).is_err());
        // a long at 0.05 with a 50% SL would trigger below one tick
        assert!(spec(SlKind::SlM).levels(Side::Buy, 0.05).is_err());
    }

    #[test]
    fn ids_are_short_and_alphanumeric() {
        let s = to_base36(now_ms());
        assert!(s.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(format!("L{}{}", s, to_base36(99)).len() <= 18);
    }
}
