//! Full request handling (parsing, `_meta` validation, dispatch, error
//! mapping) must never panic and must always produce valid JSON.
#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use flymcprust_core::{Server, TransportContext};
use tokio_util::sync::CancellationToken;

fn server() -> &'static Server {
    static SERVER: OnceLock<Server> = OnceLock::new();
    SERVER.get_or_init(|| match Server::builder("fuzz", "0.0.0").build() {
        Ok(server) => server,
        Err(error) => panic!("fuzz server: {error}"),
    })
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => panic!("fuzz runtime: {error}"),
    })
}

fuzz_target!(|data: &[u8]| {
    let response = runtime().block_on(server().handle_bytes(
        data,
        TransportContext::default(),
        CancellationToken::new(),
    ));
    if let Some(response) = response {
        let bytes = response.to_vec();
        assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_ok());
    }
});
