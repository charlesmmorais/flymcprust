//! The JSON-RPC envelope parser (size limit, depth scan, serde) must never
//! panic, whatever the input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use sdkmcprust_core::Limits;
use sdkmcprust_core::jsonrpc;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    if let Err(jsonrpc::Rejection::Error { id, error }) = jsonrpc::parse(data, &limits) {
        // Error responses must always serialize to valid JSON.
        let bytes = jsonrpc::Response::error(id, error).to_vec();
        assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_ok());
    }
});
