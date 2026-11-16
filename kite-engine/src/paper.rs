//! A pretend broker kept in memory. No network, no money. It is the default,
//! and it is what the SL engine will be tested against.
//!
//! MARKET orders fill at once at the price set with `set_ltp` (100.0 if none).
//! LIMIT, SL and SL-M orders rest until `simulate_fill` or `cancel_order`.

use crate::broker::Broker;
use crate::error::{Error, Result};
use crate::types::{ModifyRequest, Order, OrderRequest, OrderType, Position, Positions, Side, Variety};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

/// Faults to inject on an order with a given tag, to test the failure paths.
#[derive(Debug, Clone)]
pub enum Failure {
    /// The broker refuses every order with this tag.
    Reject(String),
    /// The next `n` attempts fail before anything is sent.
    Network(u32),
    /// The next `n` attempts DO place the order but the reply is lost
    /// (a timeout): the outcome looks unknown to the caller.
    UnknownAfterPlace(u32),
}

#[derive(Default)]
struct State {
    next: u64,
    orders: Vec<Order>,
    ltp: HashMap<String, f64>,
    failures: HashMap<String, Failure>,
}

#[derive(Default)]
pub struct PaperBroker {
    inner: Mutex<State>,
}

impl PaperBroker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_ltp(&self, symbol: &str, price: f64) {
        self.inner.lock().unwrap().ltp.insert(symbol.to_string(), price);
    }

    pub fn inject(&self, tag: &str, f: Failure) {
        self.inner.lock().unwrap().failures.insert(tag.to_string(), f);
    }

    /// Part-fill a resting order: `filled` units so far, at running average `avg`.
    pub fn simulate_partial(&self, order_id: &str, filled: u32, avg: f64) -> Result<()> {
        let mut s = self.inner.lock().unwrap();
        let o = s
            .orders
            .iter_mut()
            .find(|o| o.order_id == order_id)
            .ok_or_else(|| Error::NotFound(order_id.to_string()))?;
        if !o.is_open() || filled == 0 || filled >= o.quantity {
            return Err(Error::Validation("partial fill must be above 0 and below the quantity".into()));
        }
        o.filled_quantity = filled;
        o.pending_quantity = o.quantity - filled;
        o.average_price = avg;
        Ok(())
    }

    /// Fill a resting order completely at `price`, as the exchange would.
    pub fn simulate_fill(&self, order_id: &str, price: f64) -> Result<()> {
        let mut s = self.inner.lock().unwrap();
        let o = s
            .orders
            .iter_mut()
            .find(|o| o.order_id == order_id)
            .ok_or_else(|| Error::NotFound(order_id.to_string()))?;
        if !o.is_open() {
            return Err(Error::Api { kind: "OrderException".into(), message: format!("order is {}", o.status) });
        }
        o.status = "COMPLETE".into();
        o.filled_quantity = o.quantity;
        o.pending_quantity = 0;
        o.average_price = price;
        let sym = o.tradingsymbol.clone();
        s.ltp.insert(sym, price);
        Ok(())
    }

    fn find_open<'a>(s: &'a mut State, id: &str) -> Result<&'a mut Order> {
        let o = s.orders.iter_mut().find(|o| o.order_id == id).ok_or_else(|| Error::NotFound(id.to_string()))?;
        if !o.is_open() {
            return Err(Error::Api {
                kind: "OrderException".into(),
                message: format!("order is {} and cannot be changed", o.status),
            });
        }
        Ok(o)
    }
}

#[async_trait]
impl Broker for PaperBroker {
    fn name(&self) -> &'static str {
        "paper"
    }

    async fn place_order(&self, r: &OrderRequest) -> Result<String> {
        r.validate()?;
        let mut s = self.inner.lock().unwrap();
        let mut lose_reply = false;
        if let Some(tag) = &r.tag {
            match s.failures.get_mut(tag) {
                Some(Failure::Reject(msg)) => {
                    return Err(Error::Api { kind: "MarginException".into(), message: msg.clone() });
                }
                Some(Failure::Network(n)) if *n > 0 => {
                    *n -= 1;
                    return Err(Error::Network("injected: connection refused".into()));
                }
                Some(Failure::UnknownAfterPlace(n)) if *n > 0 => {
                    *n -= 1;
                    lose_reply = true;
                }
                _ => {}
            }
        }
        s.next += 1;
        let id = format!("PAPER-{}", s.next);
        let mut o = Order {
            order_id: id.clone(),
            exchange: r.exchange.clone(),
            tradingsymbol: r.tradingsymbol.clone(),
            transaction_type: r.side.as_str().into(),
            order_type: r.order_type.as_str().into(),
            product: r.product.as_str().into(),
            variety: r.variety.as_str().into(),
            validity: r.validity.map(|v| v.as_str()).unwrap_or("DAY").into(),
            quantity: r.quantity,
            pending_quantity: r.quantity,
            price: r.price.unwrap_or(0.0),
            trigger_price: r.trigger_price.unwrap_or(0.0),
            tag: r.tag.clone(),
            ..Default::default()
        };
        match r.order_type {
            OrderType::Market => {
                let px = s.ltp.get(&r.tradingsymbol).copied().unwrap_or(100.0);
                o.status = "COMPLETE".into();
                o.filled_quantity = r.quantity;
                o.pending_quantity = 0;
                o.average_price = px;
                s.ltp.insert(r.tradingsymbol.clone(), px);
            }
            OrderType::Limit => o.status = "OPEN".into(),
            OrderType::Sl | OrderType::SlM => o.status = "TRIGGER PENDING".into(),
        }
        s.orders.push(o);
        if lose_reply {
            return Err(Error::Unknown("injected: timeout after the order was placed".into()));
        }
        Ok(id)
    }

    async fn modify_order(&self, _v: Variety, order_id: &str, m: &ModifyRequest) -> Result<String> {
        m.validate()?;
        let mut s = self.inner.lock().unwrap();
        let o = Self::find_open(&mut s, order_id)?;
        if let Some(q) = m.quantity {
            o.quantity = q;
            o.pending_quantity = q;
        }
        if let Some(p) = m.price {
            o.price = p;
        }
        if let Some(t) = m.trigger_price {
            o.trigger_price = t;
        }
        if let Some(t) = m.order_type {
            o.order_type = t.as_str().into();
        }
        Ok(order_id.to_string())
    }

    async fn cancel_order(&self, _v: Variety, order_id: &str) -> Result<String> {
        let mut s = self.inner.lock().unwrap();
        let o = Self::find_open(&mut s, order_id)?;
        o.status = "CANCELLED".into();
        Ok(order_id.to_string())
    }

    async fn orders(&self) -> Result<Vec<Order>> {
        Ok(self.inner.lock().unwrap().orders.clone())
    }

    async fn order_history(&self, order_id: &str) -> Result<Vec<Order>> {
        let s = self.inner.lock().unwrap();
        s.orders
            .iter()
            .find(|o| o.order_id == order_id)
            .map(|o| vec![o.clone()])
            .ok_or_else(|| Error::NotFound(order_id.to_string()))
    }

    async fn positions(&self) -> Result<Positions> {
        let s = self.inner.lock().unwrap();
        let mut map: HashMap<(String, String, String), Position> = HashMap::new();
        for o in s.orders.iter().filter(|o| o.filled_quantity > 0) {
            let key = (o.exchange.clone(), o.tradingsymbol.clone(), o.product.clone());
            let p = map.entry(key).or_insert_with(|| Position {
                tradingsymbol: o.tradingsymbol.clone(),
                exchange: o.exchange.clone(),
                product: o.product.clone(),
                ..Default::default()
            });
            let qty = o.filled_quantity as i64;
            let value = o.average_price * o.filled_quantity as f64;
            if o.transaction_type == Side::Buy.as_str() {
                p.buy_quantity += qty;
                p.buy_value += value;
            } else {
                p.sell_quantity += qty;
                p.sell_value += value;
            }
        }
        let mut net: Vec<Position> = map
            .into_values()
            .map(|mut p| {
                p.quantity = p.buy_quantity - p.sell_quantity;
                p.last_price = s.ltp.get(&p.tradingsymbol).copied().unwrap_or(0.0);
                p.average_price = if p.quantity > 0 && p.buy_quantity > 0 {
                    p.buy_value / p.buy_quantity as f64
                } else if p.quantity < 0 && p.sell_quantity > 0 {
                    p.sell_value / p.sell_quantity as f64
                } else {
                    0.0
                };
                p.pnl = p.sell_value - p.buy_value + p.quantity as f64 * p.last_price;
                p
            })
            .collect();
        net.sort_by(|a, b| a.tradingsymbol.cmp(&b.tradingsymbol));
        Ok(Positions { day: net.clone(), net })
    }

    async fn margins(&self) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "equity": { "enabled": true, "net": 1_000_000.0,
                        "available": { "cash": 1_000_000.0 }, "utilised": { "debits": 0.0 } }
        }))
    }
}
