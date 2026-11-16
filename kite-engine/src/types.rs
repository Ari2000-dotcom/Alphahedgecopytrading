//! Request and response shapes, written to match Kite Connect v3.

use crate::error::{Error, Result};
use serde::{Deserialize, Deserializer, Serialize};

fn null_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    Buy,
    Sell,
}
impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        }
    }
    pub fn opposite(self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    #[serde(rename = "MARKET")]
    Market,
    #[serde(rename = "LIMIT")]
    Limit,
    #[serde(rename = "SL")]
    Sl,
    #[serde(rename = "SL-M")]
    SlM,
}
impl OrderType {
    pub fn as_str(self) -> &'static str {
        match self {
            OrderType::Market => "MARKET",
            OrderType::Limit => "LIMIT",
            OrderType::Sl => "SL",
            OrderType::SlM => "SL-M",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Product {
    Cnc,
    Nrml,
    Mis,
}
impl Product {
    pub fn as_str(self) -> &'static str {
        match self {
            Product::Cnc => "CNC",
            Product::Nrml => "NRML",
            Product::Mis => "MIS",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Validity {
    Day,
    Ioc,
}
impl Validity {
    pub fn as_str(self) -> &'static str {
        match self {
            Validity::Day => "DAY",
            Validity::Ioc => "IOC",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variety {
    #[default]
    Regular,
    Amo,
    Iceberg,
}
impl Variety {
    pub fn as_str(self) -> &'static str {
        match self {
            Variety::Regular => "regular",
            Variety::Amo => "amo",
            Variety::Iceberg => "iceberg",
        }
    }
}

fn valid_price(p: f64) -> bool {
    p.is_finite() && p > 0.0
}

/// A new order. `price` and `trigger_price` are in rupees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderRequest {
    pub exchange: String,
    pub tradingsymbol: String,
    pub side: Side,
    pub order_type: OrderType,
    pub product: Product,
    pub quantity: u32,
    #[serde(default)]
    pub price: Option<f64>,
    #[serde(default)]
    pub trigger_price: Option<f64>,
    #[serde(default)]
    pub validity: Option<Validity>,
    #[serde(default)]
    pub variety: Variety,
    /// Your own id, up to 20 letters and digits. Used to spot duplicates and
    /// to find the order again after a crash.
    #[serde(default)]
    pub tag: Option<String>,
    /// Percent. Zerodha wants this on market orders for some instruments.
    #[serde(default)]
    pub market_protection: Option<f64>,
}

impl OrderRequest {
    pub fn validate(&self) -> Result<()> {
        let bad = |m: &str| Err(Error::Validation(m.to_string()));
        if self.exchange.trim().is_empty() || self.tradingsymbol.trim().is_empty() {
            return bad("exchange and tradingsymbol are required");
        }
        if self.quantity == 0 {
            return bad("quantity must be above zero");
        }
        for (name, v) in [("price", self.price), ("trigger_price", self.trigger_price)] {
            if let Some(p) = v {
                if !valid_price(p) {
                    return Err(Error::Validation(format!("{name} must be a positive number")));
                }
            }
        }
        match self.order_type {
            OrderType::Market => {
                if self.price.is_some() || self.trigger_price.is_some() {
                    return bad("a MARKET order takes no price or trigger_price");
                }
            }
            OrderType::Limit => {
                if self.price.is_none() {
                    return bad("a LIMIT order needs price");
                }
                if self.trigger_price.is_some() {
                    return bad("a LIMIT order takes no trigger_price");
                }
            }
            OrderType::Sl => {
                if self.price.is_none() || self.trigger_price.is_none() {
                    return bad("an SL order needs price and trigger_price");
                }
            }
            OrderType::SlM => {
                if self.trigger_price.is_none() {
                    return bad("an SL-M order needs trigger_price");
                }
                if self.price.is_some() {
                    return bad("an SL-M order takes no price");
                }
            }
        }
        if let Some(tag) = &self.tag {
            if tag.is_empty() || tag.len() > 20 || !tag.chars().all(|c| c.is_ascii_alphanumeric()) {
                return bad("tag must be 1 to 20 letters and digits");
            }
        }
        if let Some(mp) = self.market_protection {
            if !(mp.is_finite() && mp >= 0.0 && mp <= 100.0) {
                return bad("market_protection must be between 0 and 100");
            }
        }
        Ok(())
    }
}

/// Changes to an open order. Only the fields you set are sent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModifyRequest {
    #[serde(default)]
    pub quantity: Option<u32>,
    #[serde(default)]
    pub price: Option<f64>,
    #[serde(default)]
    pub trigger_price: Option<f64>,
    #[serde(default)]
    pub order_type: Option<OrderType>,
    #[serde(default)]
    pub validity: Option<Validity>,
}

impl ModifyRequest {
    pub fn validate(&self) -> Result<()> {
        if self.quantity.is_none()
            && self.price.is_none()
            && self.trigger_price.is_none()
            && self.order_type.is_none()
            && self.validity.is_none()
        {
            return Err(Error::Validation("nothing to modify".into()));
        }
        if self.quantity == Some(0) {
            return Err(Error::Validation("quantity must be above zero".into()));
        }
        for (name, v) in [("price", self.price), ("trigger_price", self.trigger_price)] {
            if let Some(p) = v {
                if !valid_price(p) {
                    return Err(Error::Validation(format!("{name} must be a positive number")));
                }
            }
        }
        Ok(())
    }
}

/// One row of the order book.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Order {
    #[serde(deserialize_with = "null_default")]
    pub order_id: String,
    pub exchange_order_id: Option<String>,
    pub parent_order_id: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub status: String,
    pub status_message: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub exchange: String,
    #[serde(deserialize_with = "null_default")]
    pub tradingsymbol: String,
    #[serde(deserialize_with = "null_default")]
    pub transaction_type: String,
    #[serde(deserialize_with = "null_default")]
    pub order_type: String,
    #[serde(deserialize_with = "null_default")]
    pub product: String,
    #[serde(deserialize_with = "null_default")]
    pub variety: String,
    #[serde(deserialize_with = "null_default")]
    pub validity: String,
    #[serde(deserialize_with = "null_default")]
    pub quantity: u32,
    #[serde(deserialize_with = "null_default")]
    pub filled_quantity: u32,
    #[serde(deserialize_with = "null_default")]
    pub pending_quantity: u32,
    #[serde(deserialize_with = "null_default")]
    pub price: f64,
    #[serde(deserialize_with = "null_default")]
    pub trigger_price: f64,
    #[serde(deserialize_with = "null_default")]
    pub average_price: f64,
    pub tag: Option<String>,
    pub order_timestamp: Option<String>,
}

impl Order {
    /// Still on the exchange and changeable.
    pub fn is_open(&self) -> bool {
        matches!(
            self.status.as_str(),
            "OPEN" | "TRIGGER PENDING" | "PUT ORDER REQ RECEIVED" | "VALIDATION PENDING" | "MODIFY PENDING"
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Position {
    #[serde(deserialize_with = "null_default")]
    pub tradingsymbol: String,
    #[serde(deserialize_with = "null_default")]
    pub exchange: String,
    #[serde(deserialize_with = "null_default")]
    pub product: String,
    #[serde(deserialize_with = "null_default")]
    pub quantity: i64,
    #[serde(deserialize_with = "null_default")]
    pub average_price: f64,
    #[serde(deserialize_with = "null_default")]
    pub last_price: f64,
    #[serde(deserialize_with = "null_default")]
    pub pnl: f64,
    #[serde(deserialize_with = "null_default")]
    pub buy_quantity: i64,
    #[serde(deserialize_with = "null_default")]
    pub sell_quantity: i64,
    #[serde(deserialize_with = "null_default")]
    pub buy_value: f64,
    #[serde(deserialize_with = "null_default")]
    pub sell_value: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Positions {
    pub net: Vec<Position>,
    pub day: Vec<Position>,
}

/// Round a price to the exchange tick (0.05 for options). `up` rounds away
/// from zero. A short option's SL buy trigger rounds up; a long's SL sell
/// trigger rounds down.
pub fn round_to_tick(price: f64, tick: f64, up: bool) -> f64 {
    let steps = price / tick;
    let steps = if up { (steps - 1e-9).ceil() } else { (steps + 1e-9).floor() };
    ((steps * tick) * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> OrderRequest {
        OrderRequest {
            exchange: "NFO".into(),
            tradingsymbol: "NIFTY2611224500CE".into(),
            side: Side::Sell,
            order_type: OrderType::Market,
            product: Product::Nrml,
            quantity: 75,
            price: None,
            trigger_price: None,
            validity: None,
            variety: Variety::Regular,
            tag: None,
            market_protection: None,
        }
    }

    #[test]
    fn market_order_is_valid() {
        assert!(base().validate().is_ok());
    }

    #[test]
    fn limit_needs_price() {
        let mut r = base();
        r.order_type = OrderType::Limit;
        assert!(r.validate().is_err());
        r.price = Some(100.0);
        assert!(r.validate().is_ok());
    }

    #[test]
    fn sl_needs_both_and_slm_forbids_price() {
        let mut r = base();
        r.order_type = OrderType::Sl;
        r.trigger_price = Some(150.0);
        assert!(r.validate().is_err());
        r.price = Some(152.0);
        assert!(r.validate().is_ok());
        r.order_type = OrderType::SlM;
        assert!(r.validate().is_err());
        r.price = None;
        assert!(r.validate().is_ok());
    }

    #[test]
    fn rejects_zero_quantity_and_bad_tag_and_nan() {
        let mut r = base();
        r.quantity = 0;
        assert!(r.validate().is_err());
        let mut r = base();
        r.tag = Some("has space".into());
        assert!(r.validate().is_err());
        r.tag = Some("a".repeat(21));
        assert!(r.validate().is_err());
        r.tag = Some("AH12abc".into());
        assert!(r.validate().is_ok());
        let mut r = base();
        r.order_type = OrderType::Limit;
        r.price = Some(f64::NAN);
        assert!(r.validate().is_err());
    }

    #[test]
    fn modify_must_change_something() {
        assert!(ModifyRequest::default().validate().is_err());
        let m = ModifyRequest { price: Some(101.0), ..Default::default() };
        assert!(m.validate().is_ok());
    }

    #[test]
    fn tick_rounding() {
        assert_eq!(round_to_tick(150.0, 0.05, true), 150.0);
        assert_eq!(round_to_tick(150.01, 0.05, true), 150.05);
        assert_eq!(round_to_tick(150.04, 0.05, false), 150.0);
        assert_eq!(round_to_tick(101.0 * 1.5, 0.05, true), 151.5);
        assert_eq!(round_to_tick(99.5 * 1.5, 0.05, true), 149.25);
    }

    #[test]
    fn order_parses_nulls_and_missing_fields() {
        let j = r#"{"order_id":"1","status":"OPEN","status_message":null,"price":100.5,"trigger_price":null,"tag":null,"quantity":75}"#;
        let o: Order = serde_json::from_str(j).unwrap();
        assert_eq!(o.order_id, "1");
        assert_eq!(o.price, 100.5);
        assert_eq!(o.trigger_price, 0.0);
        assert!(o.is_open());
    }
}
