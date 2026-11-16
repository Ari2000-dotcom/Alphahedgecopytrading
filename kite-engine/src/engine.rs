//! The gate in front of a broker: validates, applies risk limits, and refuses
//! duplicates, then passes the call on.

use crate::broker::Broker;
use crate::error::{Error, Result};
use crate::types::{ModifyRequest, Order, OrderRequest, Positions, Variety};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct RiskLimits {
    /// Largest quantity in one order, in contract units (not lots).
    pub max_quantity: u32,
}

impl Default for RiskLimits {
    fn default() -> Self {
        // 1800 is NIFTY's exchange freeze quantity: a bigger order would be
        // refused by the exchange anyway.
        Self { max_quantity: 1800 }
    }
}

pub struct Engine {
    broker: Arc<dyn Broker>,
    risk: RiskLimits,
    seen_tags: Mutex<HashSet<String>>,
}

impl Engine {
    pub fn new(broker: Arc<dyn Broker>, risk: RiskLimits) -> Self {
        Self { broker, risk, seen_tags: Mutex::new(HashSet::new()) }
    }

    /// Allow a tag to be used again. Call it only after reading the order book
    /// and confirming the broker has no order with that tag.
    pub fn forget_tag(&self, tag: &str) {
        self.seen_tags.lock().unwrap().remove(tag);
    }

    pub fn broker_name(&self) -> &'static str {
        self.broker.name()
    }

    pub async fn place(&self, req: &OrderRequest) -> Result<String> {
        req.validate()?;
        if req.quantity > self.risk.max_quantity {
            return Err(Error::Risk(format!(
                "quantity {} is above the limit of {}",
                req.quantity, self.risk.max_quantity
            )));
        }
        if let Some(tag) = &req.tag {
            if !self.seen_tags.lock().unwrap().insert(tag.clone()) {
                return Err(Error::Duplicate(tag.clone()));
            }
        }
        let res = self.broker.place_order(req).await;
        // The broker clearly said no, or nothing was sent: the same tag may be
        // tried again. After an unknown outcome the tag stays used, so a retry
        // can't create a second order.
        if matches!(res, Err(Error::Api { .. }) | Err(Error::Network(_)) | Err(Error::Validation(_))) {
            if let Some(tag) = &req.tag {
                self.seen_tags.lock().unwrap().remove(tag);
            }
        }
        res
    }

    pub async fn modify(&self, variety: Variety, id: &str, m: &ModifyRequest) -> Result<String> {
        m.validate()?;
        if let Some(q) = m.quantity {
            if q > self.risk.max_quantity {
                return Err(Error::Risk(format!("quantity {q} is above the limit of {}", self.risk.max_quantity)));
            }
        }
        self.broker.modify_order(variety, id, m).await
    }

    pub async fn cancel(&self, variety: Variety, id: &str) -> Result<String> {
        self.broker.cancel_order(variety, id).await
    }

    pub async fn orders(&self) -> Result<Vec<Order>> {
        self.broker.orders().await
    }

    pub async fn order_history(&self, id: &str) -> Result<Vec<Order>> {
        self.broker.order_history(id).await
    }

    pub async fn positions(&self) -> Result<Positions> {
        self.broker.positions().await
    }

    pub async fn margins(&self) -> Result<serde_json::Value> {
        self.broker.margins().await
    }
}
