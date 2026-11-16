//! One error type for the whole engine.
//!
//! `Unknown` matters most: a timeout while *sending* an order means the broker
//! may or may not have received it. It must never be retried blindly; read the
//! order book first.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid request: {0}")]
    Validation(String),
    #[error("rejected by risk limit: {0}")]
    Risk(String),
    #[error("duplicate order tag: {0}")]
    Duplicate(String),
    /// The broker answered and said no (margin, price band, bad input...).
    #[error("broker rejected ({kind}): {message}")]
    Api { kind: String, message: String },
    /// Nothing was sent: the connection could not be made.
    #[error("network error, nothing was sent: {0}")]
    Network(String),
    /// A write may or may not have reached the broker. Check the order book.
    #[error("outcome unknown, check the order book before retrying: {0}")]
    Unknown(String),
    #[error("unexpected reply from broker: {0}")]
    BadResponse(String),
    #[error("not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;
