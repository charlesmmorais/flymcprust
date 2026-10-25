//! Transports. Each one only moves bytes; all protocol logic lives in
//! [`Server`](crate::Server).

#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "http")]
mod oauth;
#[cfg(feature = "stdio")]
pub mod stdio;
