//! The contract every broker adapter implements. Zerodha is the first; each
//! further broker is one more implementation of this trait.

use crate::error::Result;
use crate::types::{ModifyRequest, Order, OrderRequest, Positions, Variety};
use async_trait::async_trait;

#[async_trait]
pub trait Broker: Send + Sync {
    fn name(&self) -> &'static str;

    /// Returns the broker's order id. An id only means the order was accepted
    /// for processing, not that it filled.
    async fn place_order(&self, req: &OrderRequest) -> Result<String>;
    async fn modify_order(&self, variety: Variety, order_id: &str, req: &ModifyRequest) -> Result<String>;
    async fn cancel_order(&self, variety: Variety, order_id: &str) -> Result<String>;

    /// Today's order book.
    async fn orders(&self) -> Result<Vec<Order>>;
    /// Every state one order has been through, oldest first.
    async fn order_history(&self, order_id: &str) -> Result<Vec<Order>>;
    async fn positions(&self) -> Result<Positions>;
    async fn margins(&self) -> Result<serde_json::Value>;
}
