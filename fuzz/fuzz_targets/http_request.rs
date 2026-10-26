//! The Streamable HTTP transport must never panic, whatever the headers and
//! body: Origin and Host checks, OAuth bearer parsing, legacy session
//! tokens, MCP header validation and SSE streaming all run on client input.
//!
//! Input: an optional first line `METHOD path`, then `name: value` header
//! lines, an empty line, and the body.
#![no_main]

use std::sync::OnceLock;

use bytes::Bytes;
use flymcprust_core::transport::http::{HttpConfig, OAuth, StreamableHttpService};
use flymcprust_core::{Principal, Server, TokenError};
use http_body_util::{BodyExt as _, Full};
use libfuzzer_sys::fuzz_target;
use tower_service::Service as _;

fn verify(token: &str) -> Result<Principal, TokenError> {
    if token == "valid" {
        Ok(Principal::new("fuzzer").with_scopes(["mcp"]))
    } else {
        Err(TokenError::new("unknown"))
    }
}

fn services() -> &'static [StreamableHttpService; 2] {
    static SERVICES: OnceLock<[StreamableHttpService; 2]> = OnceLock::new();
    SERVICES.get_or_init(|| {
        let server = Server::builder("fuzz", "0.0.0")
            .build()
            .expect("fuzz server");
        let oauth = OAuth::builder("http://localhost/mcp", verify)
            .authorization_server("http://localhost:9000")
            .required_scopes(["mcp"])
            .build()
            .expect("fuzz oauth");
        [
            StreamableHttpService::new(server.clone()),
            StreamableHttpService::with_config(server, HttpConfig::new().oauth(oauth)),
        ]
    })
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("fuzz runtime")
    })
}

fuzz_target!(|data: &[u8]| {
    let Some((&selector, data)) = data.split_first() else {
        return;
    };
    let (head, body) = match data.windows(2).position(|w| w == b"\n\n") {
        Some(i) => (&data[..i], &data[i + 2..]),
        None => (data, &[][..]),
    };
    let Ok(head) = std::str::from_utf8(head) else {
        return;
    };
    let mut lines = head.lines();
    let mut builder = http::Request::builder().method("POST").uri("/mcp");
    if let Some(first) = lines.next() {
        match first.split_once(' ') {
            Some((method, path)) if !first.contains(':') => {
                builder = builder.method(method).uri(path);
            }
            _ => {
                if let Some((name, value)) = first.split_once(':') {
                    builder = builder.header(name.trim(), value.trim());
                }
            }
        }
    }
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            builder = builder.header(name.trim(), value.trim());
        }
    }
    let Ok(request) = builder.body(Full::new(Bytes::copy_from_slice(body))) else {
        return;
    };
    let mut service = services()[usize::from(selector & 1)].clone();
    runtime().block_on(async {
        let response = service.call(request).await.expect("infallible");
        // Drain streaming bodies too, with a deadline (legacy input requests
        // wait for client answers).
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            response.into_body().collect(),
        )
        .await;
    });
});
