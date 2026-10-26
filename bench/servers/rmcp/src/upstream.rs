//! HTTP client for the simulated upstream service (`BENCH_UPSTREAM`,
//! default `127.0.0.1:9100`). The same code is used by every Rust server.
use std::sync::OnceLock;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Empty};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;

type Upstream = Client<HttpConnector, Empty<Bytes>>;

fn client() -> &'static (Upstream, String) {
    static CLIENT: OnceLock<(Upstream, String)> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let authority =
            std::env::var("BENCH_UPSTREAM").unwrap_or_else(|_| "127.0.0.1:9100".to_owned());
        let mut connector = HttpConnector::new();
        connector.set_nodelay(true);
        let client = Client::builder(TokioExecutor::new())
            .pool_max_idle_per_host(4096)
            .build(connector);
        (client, authority)
    })
}

/// Fetches `/item/{id}` and returns the body.
pub async fn get(id: u64) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let (client, authority) = client();
    let uri = format!("http://{authority}/item/{id}").parse()?;
    let response = client.get(uri).await?;
    let body = response.into_body().collect().await?.to_bytes();
    Ok(String::from_utf8_lossy(&body).into_owned())
}
