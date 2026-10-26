//! The real-world suite (`mcp-load real`): FlyMCPRust and rmcp serving the
//! same 203 tools (200 generated, a complex order, an upstream call), with
//! and without authentication, under workloads closer to production:
//!
//! - `tools_list_200`: listing 200 tools (a ~100 KB response);
//! - `call_last_of_200`: calling the last of 200 tools;
//! - `call_order_complex`: a nested ~10 KB order with 40 lines;
//! - `call_add_many_clients`: 1,024 and 4,096 keep-alive connections;
//! - `call_add_no_keepalive`: a new connection for every request;
//! - `call_fetch_upstream`: each call waits for an HTTP upstream that takes
//!   20 ms (simulated in this process), at 64, 256 and 1,024 clients;
//! - `mix`: 50% trivial calls, 20% orders, 20% upstream calls, 10% listing;
//!   `mix_without_upstream` replaces the upstream calls by trivial calls.
//!
//! The authenticated targets send an RS256 JWT with every request; both
//! SDKs check it with the same verifier, so the comparison isolates each
//! SDK's own overhead.

use std::convert::Infallible;

use hyper::Response;
use hyper::header::{CONNECTION, HeaderValue};

use super::{Arc, Command, Duration, Histogram, Instant, Path, PathBuf, Stdio, Value, json};
use super::{
    Driver, Error, Measurement, Options, Prepared, Probe, Running, Scenario, ServerInfo,
    WorkerStats, dir_size, exe, machine, wait_ready,
};

/// Address of the simulated upstream service.
const UPSTREAM: &str = "127.0.0.1:9100";

/// One request kind of a workload.
struct Request {
    prepared: Prepared,
    /// A response containing these bytes counts as an error.
    reject: Option<&'static [u8]>,
}

/// What the clients send, and how.
struct Workload {
    name: &'static str,
    /// Requests in the order each client cycles through them.
    requests: Vec<(Scenario, Option<&'static [u8]>)>,
    keepalive: bool,
    levels: &'static [usize],
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

/// A deterministic order with 40 lines (~10 KB of JSON).
fn order() -> Value {
    let kinds = ["physical", "digital", "service"];
    let items: Vec<Value> = (0..40)
        .map(|i| {
            json!({
                "sku": format!("SKU-{:05}", 1000 + i * 37),
                "quantity": 1 + i % 7,
                "unit_price": 9.5 + f64::from(i) * 1.25,
                "kind": kinds[(i % 3) as usize],
                "attributes": [format!("color-{}", i % 5), format!("size-{}", i % 4), "eco".to_owned()],
            })
        })
        .collect();
    json!({
        "order": {
            "id": "ORD-2026-000123",
            "customer": {
                "id": 4_815_162_342_u64,
                "name": "Comercial Exemplo Ltda",
                "email": "compras@exemplo.com.br",
                "address": {
                    "street": "Rua das Flores, 123",
                    "city": "Brasília",
                    "postal_code": "70000-000",
                    "country": "BR",
                },
                "tags": ["wholesale", "priority", "net-30"],
            },
            "items": items,
            "notes": "Deliver to the loading dock; invoice by e-mail.",
        }
    })
}

fn workloads(root: &Path, quick: bool) -> Result<Vec<Workload>, Error> {
    let last: Value = serde_json::from_str(&std::fs::read_to_string(
        root.join("bench").join("fixtures").join("last_tool.json"),
    )?)?;
    let last_name = leak(
        last["name"]
            .as_str()
            .ok_or("last_tool.json: no name")?
            .to_owned(),
    );
    let call = |name: &'static str, tool: &'static str, arguments: Value| Scenario {
        name,
        method: "tools/call",
        tool: Some(tool),
        arguments,
    };
    let list = Scenario {
        name: "tools_list_200",
        method: "tools/list",
        tool: None,
        arguments: Value::Null,
    };
    let add = call("call_add", "add", json!({ "a": 1.5, "b": 2.25 }));
    let complex = call("call_order_complex", "order", order());
    let fetch = call("call_fetch_upstream", "fetch", json!({ "id": 42 }));
    let last_call = call("call_last_of_200", last_name, last["arguments"].clone());
    let unavailable: Option<&'static [u8]> = Some(b"upstream unavailable");
    let (one, high, io, mix): (
        &'static [usize],
        &'static [usize],
        &'static [usize],
        &'static [usize],
    ) = if quick {
        (&[16], &[256], &[64], &[64])
    } else {
        (&[64], &[1024, 4096], &[64, 256, 1024], &[64, 1024])
    };
    let mut mixed = Vec::new();
    for _ in 0..5 {
        mixed.push((add.clone(), None));
    }
    for _ in 0..2 {
        mixed.push((complex.clone(), None));
        mixed.push((fetch.clone(), unavailable));
    }
    mixed.push((list.clone(), None));
    // The same mix with the upstream calls replaced by trivial calls.
    let mixed_cpu: Vec<_> = mixed
        .iter()
        .map(|(s, _)| {
            if s.tool == Some("fetch") {
                (add.clone(), None)
            } else {
                (s.clone(), None)
            }
        })
        .collect();
    let single = |name: &'static str, s: &Scenario, reject, keepalive, levels| Workload {
        name,
        requests: vec![(s.clone(), reject)],
        keepalive,
        levels,
    };
    Ok(vec![
        single("tools_list_200", &list, None, true, one),
        single("call_last_of_200", &last_call, None, true, one),
        single("call_order_complex", &complex, None, true, one),
        single("call_add_many_clients", &add, None, true, high),
        single("call_add_no_keepalive", &add, None, false, one),
        single("call_fetch_upstream", &fetch, unavailable, true, io),
        Workload {
            name: "mix",
            requests: mixed,
            keepalive: true,
            levels: mix,
        },
        Workload {
            name: "mix_without_upstream",
            requests: mixed_cpu,
            keepalive: true,
            levels: mix,
        },
    ])
}

/// One closed-loop client cycling through the workload's requests.
async fn worker(
    authority: Arc<str>,
    requests: Arc<[Request]>,
    mut next: usize,
    keepalive: bool,
    record_from: Instant,
    until: Instant,
) -> WorkerStats {
    let mut stats = WorkerStats::new();
    'connect: while Instant::now() < until {
        let Ok(stream) = tokio::net::TcpStream::connect(&*authority).await else {
            stats.errors += 1;
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        };
        let _ = stream.set_nodelay(true);
        let Ok((mut sender, connection)) =
            hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await
        else {
            stats.errors += 1;
            continue;
        };
        tokio::spawn(connection);
        while Instant::now() < until {
            let Some(request) = requests.get(next % requests.len()) else {
                return stats;
            };
            next += 1;
            if sender.ready().await.is_err() {
                stats.errors += 1;
                continue 'connect;
            }
            let mut http = request.prepared.request(&authority);
            if !keepalive {
                // The server closes first, so TIME_WAIT stays on its side.
                http.headers_mut()
                    .insert(CONNECTION, HeaderValue::from_static("close"));
            }
            let started = Instant::now();
            let ok = match sender.send_request(http).await {
                Ok(response) => {
                    let status_ok = response.status().is_success();
                    match http_body_util::BodyExt::collect(response.into_body()).await {
                        Ok(body) => {
                            let body = body.to_bytes();
                            status_ok
                                && !body.is_empty()
                                && request
                                    .reject
                                    .is_none_or(|bad| !body.windows(bad.len()).any(|w| w == bad))
                        }
                        Err(_) => false,
                    }
                }
                Err(_) => false,
            };
            stats.record(started, record_from, ok);
            if !ok || !keepalive {
                continue 'connect;
            }
        }
    }
    stats
}

#[allow(clippy::too_many_arguments)]
async fn measure(
    server: &str,
    authority: &Arc<str>,
    token: Option<&str>,
    pid: u32,
    workload: &Workload,
    concurrency: usize,
    run: usize,
    warmup: Duration,
    duration: Duration,
) -> Measurement {
    let requests: Arc<[Request]> = workload
        .requests
        .iter()
        .map(|(s, reject)| Request {
            prepared: Prepared::new(s, token),
            reject: *reject,
        })
        .collect();
    let start = Instant::now();
    let record_from = start + warmup;
    let until = record_from + duration;
    let handles: Vec<_> = (0..concurrency)
        .map(|i| {
            tokio::spawn(worker(
                Arc::clone(authority),
                Arc::clone(&requests),
                i,
                workload.keepalive,
                record_from,
                until,
            ))
        })
        .collect();

    let mut probe = Probe::new(Some(pid));
    tokio::time::sleep_until(record_from.into()).await;
    let cpu_start = probe.sample().map(|(_, cpu)| cpu);
    let mut rss_peak = 0u64;
    while Instant::now() < until {
        if let Some((rss, _)) = probe.sample() {
            rss_peak = rss_peak.max(rss);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let cpu_end = probe.sample().map(|(rss, cpu)| {
        rss_peak = rss_peak.max(rss);
        cpu
    });

    let mut merged = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).ok();
    let (mut ok, mut errors) = (0, 0);
    for handle in handles {
        if let Ok(stats) = handle.await {
            ok += stats.ok;
            errors += stats.errors;
            if let (Some(m), Some(h)) = (&mut merged, &stats.histogram) {
                let _ = m.add(h);
            }
        }
    }
    let seconds = duration.as_secs_f64();
    let q = |p: f64| merged.as_ref().map_or(0, |h| h.value_at_quantile(p));
    Measurement {
        server: server.to_owned(),
        transport: "http",
        scenario: workload.name.to_owned(),
        concurrency,
        run,
        seconds,
        requests: ok,
        errors,
        rps: ok as f64 / seconds,
        p50_us: q(0.50),
        p95_us: q(0.95),
        p99_us: q(0.99),
        max_us: merged.as_ref().map_or(0, Histogram::max),
        cpu_ms_per_1k: match (cpu_start, cpu_end) {
            (Some(a), Some(b)) if ok > 0 && b > a => Some((b - a) as f64 * 1000.0 / ok as f64),
            _ => None,
        },
        rss_peak_mib: (rss_peak > 0).then(|| rss_peak as f64 / (1024.0 * 1024.0)),
    }
}

/// The simulated upstream: answers every request after `delay`.
async fn start_upstream(delay: Duration) -> Result<(), Error> {
    let listener = tokio::net::TcpListener::bind(UPSTREAM).await?;
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let _ = stream.set_nodelay(true);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |_request| async move {
                    tokio::time::sleep(delay).await;
                    Ok::<_, Infallible>(Response::new(http_body_util::Full::new(
                        bytes::Bytes::from_static(b"{\"status\":\"ok\",\"stock\":42}"),
                    )))
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    Ok(())
}

struct RealTarget {
    name: &'static str,
    program: PathBuf,
    args: Vec<String>,
    port: u16,
    token: Option<String>,
}

fn targets(root: &Path) -> Vec<RealTarget> {
    let release = root.join("bench").join("target").join("release");
    let fixtures = root.join("bench").join("fixtures");
    let jwks = fixtures.join("jwks.json").display().to_string();
    let token = std::fs::read_to_string(fixtures.join("token.txt"))
        .ok()
        .map(|t| t.trim().to_owned());
    let target = |name, bin: &str, port: u16, extra: &[&str], auth: bool| RealTarget {
        name,
        program: release.join(exe(bin)),
        args: std::iter::once(port.to_string())
            .chain(std::iter::once("real".to_owned()))
            .chain(extra.iter().map(|s| (*s).to_owned()))
            .chain(
                auth.then(|| ["oauth".to_owned(), jwks.clone()])
                    .into_iter()
                    .flatten(),
            )
            .collect(),
        port,
        token: if auth { token.clone() } else { None },
    };
    vec![
        target("flymcprust", "bench-flymcprust", 8201, &[], false),
        target("rmcp", "bench-rmcp", 8202, &[], false),
        target("flymcprust + JWT", "bench-flymcprust", 8203, &[], true),
        target("rmcp + JWT", "bench-rmcp", 8204, &[], true),
        target(
            "flymcprust (default limits)",
            "bench-flymcprust",
            8205,
            &["defaults"],
            false,
        ),
    ]
}

/// Runs the real-world suite and writes `results.json`.
pub(crate) async fn suite(options: &Options) -> Result<(), Error> {
    let root = std::env::current_dir()?;
    let (warmup, duration) = if options.quick {
        (Duration::from_secs(1), Duration::from_secs(3))
    } else {
        (Duration::from_secs(2), Duration::from_secs(8))
    };
    let upstream_ms = 20;
    start_upstream(Duration::from_millis(upstream_ms)).await?;
    let workloads = workloads(&root, options.quick)?;
    let mut infos = Vec::new();
    let mut results = Vec::new();
    for target in targets(&root) {
        if let Some(only) = &options.only
            && !target.name.contains(only.as_str())
        {
            continue;
        }
        if !target.program.exists() {
            eprintln!(
                "skipping {}: {} not found",
                target.name,
                target.program.display()
            );
            continue;
        }
        eprintln!("== {}", target.name);
        let mut command = match &options.server_cpus {
            Some(cpus) if cfg!(target_os = "linux") => {
                let mut c = Command::new("taskset");
                c.arg("-c").arg(cpus).arg(&target.program);
                c
            }
            _ => Command::new(&target.program),
        };
        let child = command
            .args(&target.args)
            .env("BENCH_UPSTREAM", UPSTREAM)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let pid = child.id();
        let child = Running(child);
        let authority: Arc<str> = Arc::from(format!("127.0.0.1:{}", target.port));
        let driver = Driver::Http {
            authority: Arc::clone(&authority),
            token: target.token.clone(),
        };
        let startup = wait_ready(&driver, Duration::from_secs(60)).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let idle = Probe::new(Some(pid)).sample().map(|(rss, _)| rss);
        infos.push(ServerInfo {
            server: target.name.to_owned(),
            transport: "http",
            startup_ms: startup.map(|d| d.as_secs_f64() * 1000.0),
            idle_rss_mib: idle.map(|b| b as f64 / (1024.0 * 1024.0)),
            artifact_mib: Some(dir_size(&target.program) as f64 / (1024.0 * 1024.0)),
            artifact: target
                .program
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        });
        if startup.is_none() {
            eprintln!("   did not answer server/discover; skipped");
            continue;
        }
        for workload in &workloads {
            for &c in workload.levels {
                for run in 0..options.repeat {
                    let m = measure(
                        target.name,
                        &authority,
                        target.token.as_deref(),
                        pid,
                        workload,
                        c,
                        run,
                        warmup,
                        duration,
                    )
                    .await;
                    eprintln!(
                        "   {:<22} c={:<5} #{run} {:>9.0} req/s  p50 {:>7} µs  p99 {:>8} µs  errors {}",
                        m.scenario, m.concurrency, m.rps, m.p50_us, m.p99_us, m.errors
                    );
                    results.push(m);
                }
            }
        }
        drop(child);
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    std::fs::create_dir_all(&options.out)?;
    let report = json!({
        "suite": "real",
        "date": std::env::var("BENCH_DATE").ok(),
        "commit": std::env::var("BENCH_COMMIT").ok(),
        "machine": machine(options.server_cpus.as_deref()),
        "quick": options.quick,
        "repeat": options.repeat,
        "warmup_s": warmup.as_secs_f64(),
        "duration_s": duration.as_secs_f64(),
        "upstream_ms": upstream_ms,
        "servers": infos,
        "results": results,
    });
    let path = options.out.join("results.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    eprintln!("wrote {}", path.display());
    Ok(())
}
