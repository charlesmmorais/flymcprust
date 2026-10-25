//! Transports. Each one only moves bytes; all protocol logic lives in
//! [`Server`](crate::Server).

#[cfg(feature = "stdio")]
pub mod stdio;
