//! `mcp-load`: closed-loop load generator for MCP servers over Streamable
//! HTTP (2026-07-28, JSON responses).
//!
//! ```text
//! mcp-load suite [--quick] [--out DIR]      run every server and scenario
//! mcp-load run URL SCENARIO CONCURRENCY SECONDS
//! ```
//!
//! Each simulated client owns one keep-alive HTTP/1.1 connection and sends a
//! new request as soon as the previous response arrives. Latency is recorded
//! in an HDR histogram after a warm-up period; the server process's memory
//! (RSS) and CPU time are sampled through `sysinfo`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use hdrhistogram::Histogram;
use http_body_util::{BodyExt as _, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde_json::{Value, json};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// One request shape.
#[derive(Clone)]
struct Scenario {
    name: &'static str,
    method: &'static str,
    tool: Option<&'static str>,
    arguments: Value,
}

fn scenarios() -> Vec<Scenario> {
    let values: Vec<f64> = (0..100).map(|i| f64::from(i) * 1.5).collect();
    vec![
        Scenario {
            name: "tools_list",
            method: "tools/list",
            tool: None,
            arguments: Value::Null,
        },
        Scenario {
            name: "call_add",
            method: "tools/call",
            tool: Some("add"),
            arguments: json!({ "a": 1.5, "b": 2.25 }),
        },
        Scenario {
            name: "call_stats_100",
            method: "tools/call",
            tool: Some("stats"),
            arguments: json!({ "values": values }),
        },
        Scenario {
            name: "call_echo_64KiB",
            method: "tools/call",
            tool: Some("echo"),
            arguments: json!({ "text": "x".repeat(64 * 1024) }),
        },
    ]
}

/// A prepared request: body and headers built once.
struct Prepared {
    body: Bytes,
    method: &'static str,
    tool: Option<&'static str>,
}

impl Prepared {
    fn new(s: &Scenario) -> Self {
        let mut params = json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
            }
        });
        if let Some(tool) = s.tool {
            params["name"] = json!(tool);
            params["arguments"] = s.arguments.clone();
        }
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": s.method, "params": params });
        Self {
            body: Bytes::from(serde_json::to_vec(&body).unwrap_or_default()),
            method: s.method,
            tool: s.tool,
        }
    }

    fn request(&self, authority: &str) -> Request<Full<Bytes>> {
        let mut builder = Request::post("/mcp")
            .header("host", authority)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", self.method);
        if let Some(tool) = self.tool {
            builder = builder.header("mcp-name", tool);
        }
        builder
            .body(Full::new(self.body.clone()))
            .unwrap_or_else(|_| Request::new(Full::new(Bytes::new())))
    }
}

#[derive(Default)]
struct WorkerStats {
    histogram: Option<Histogram<u64>>,
    ok: u64,
    errors: u64,
}

/// One closed-loop client.
async fn worker(
    authority: Arc<str>,
    prepared: Arc<Prepared>,
    record_from: Instant,
    until: Instant,
) -> WorkerStats {
    let mut stats = WorkerStats {
        histogram: Histogram::new_with_bounds(1, 60_000_000, 3).ok(),
        ..WorkerStats::default()
    };
    'connect: while Instant::now() < until {
        let Ok(stream) = tokio::net::TcpStream::connect(&*authority).await else {
            stats.errors += 1;
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        };
        let _ = stream.set_nodelay(true);
        let Ok((mut sender, connection)) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await
        else {
            stats.errors += 1;
            continue;
        };
        tokio::spawn(connection);
        while Instant::now() < until {
            if sender.ready().await.is_err() {
                stats.errors += 1;
                continue 'connect;
            }
            let started = Instant::now();
            let outcome = match sender.send_request(prepared.request(&authority)).await {
                Ok(response) => {
                    let ok = response.status().is_success();
                    match response.into_body().collect().await {
                        Ok(body) => ok && !body.to_bytes().is_empty(),
                        Err(_) => false,
                    }
                }
                Err(_) => false,
            };
            let elapsed = started.elapsed();
            if started >= record_from {
                if outcome {
                    stats.ok += 1;
                    if let Some(h) = &mut stats.histogram {
                        let _ = h.record(
                            u64::try_from(elapsed.as_micros())
                                .unwrap_or(u64::MAX)
                                .max(1),
                        );
                    }
                } else {
                    stats.errors += 1;
                }
            }
            if !outcome {
                continue 'connect;
            }
        }
    }
    stats
}

#[derive(Serialize, Clone)]
struct Measurement {
    server: String,
    scenario: String,
    concurrency: usize,
    seconds: f64,
    requests: u64,
    errors: u64,
    rps: f64,
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    max_us: u64,
    /// Server CPU time per 1000 requests, in milliseconds.
    cpu_ms_per_1k: Option<f64>,
    /// Peak server RSS during the measurement, in MiB.
    rss_peak_mib: Option<f64>,
}

/// Samples a process's memory and CPU time.
struct Probe {
    system: System,
    pid: Option<Pid>,
}

impl Probe {
    fn new(pid: Option<u32>) -> Self {
        Self {
            system: System::new(),
            pid: pid.map(Pid::from_u32),
        }
    }

    /// (RSS bytes, accumulated CPU milliseconds) of the process and all its
    /// descendants: on Windows a venv's `python.exe` is a launcher that runs
    /// the real interpreter as a child process.
    fn sample(&mut self) -> Option<(u64, u64)> {
        let root = self.pid?;
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_memory().with_cpu(),
        );
        self.system.process(root)?;
        let mut tree = vec![root];
        let mut i = 0;
        while let Some(&parent) = tree.get(i) {
            tree.extend(
                self.system
                    .processes()
                    .iter()
                    .filter(|(_, p)| p.parent() == Some(parent))
                    .map(|(pid, _)| *pid),
            );
            i += 1;
        }
        let (mut rss, mut cpu) = (0, 0);
        for pid in tree {
            if let Some(p) = self.system.process(pid) {
                rss += p.memory();
                cpu += p.accumulated_cpu_time();
            }
        }
        Some((rss, cpu))
    }
}

async fn measure(
    server: &str,
    authority: &str,
    pid: Option<u32>,
    scenario: &Scenario,
    concurrency: usize,
    warmup: Duration,
    duration: Duration,
) -> Measurement {
    let prepared = Arc::new(Prepared::new(scenario));
    let authority: Arc<str> = Arc::from(authority);
    let start = Instant::now();
    let record_from = start + warmup;
    let until = record_from + duration;
    let handles: Vec<_> = (0..concurrency)
        .map(|_| {
            tokio::spawn(worker(
                Arc::clone(&authority),
                Arc::clone(&prepared),
                record_from,
                until,
            ))
        })
        .collect();

    // Sample the server while the load runs.
    let mut probe = Probe::new(pid);
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
        scenario: scenario.name.to_owned(),
        concurrency,
        seconds,
        requests: ok,
        errors,
        rps: ok as f64 / seconds,
        p50_us: q(0.50),
        p95_us: q(0.95),
        p99_us: q(0.99),
        max_us: merged.as_ref().map_or(0, Histogram::max),
        cpu_ms_per_1k: match (cpu_start, cpu_end) {
            (Some(a), Some(b)) if ok > 0 => Some((b.saturating_sub(a)) as f64 * 1000.0 / ok as f64),
            _ => None,
        },
        rss_peak_mib: (rss_peak > 0).then(|| rss_peak as f64 / (1024.0 * 1024.0)),
    }
}

/// A server under test.
struct Target {
    name: &'static str,
    program: PathBuf,
    args: Vec<String>,
    env: Vec<(&'static str, &'static str)>,
    port: u16,
    /// What is measured as the deployable artifact.
    artifact: PathBuf,
}

#[derive(Serialize)]
struct ServerInfo {
    server: String,
    startup_ms: Option<f64>,
    idle_rss_mib: Option<f64>,
    artifact_mib: Option<f64>,
    artifact: String,
}

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

fn targets(root: &Path) -> Vec<Target> {
    let release = root.join("bench").join("target").join("release");
    let python_dir = root.join("bench").join("servers").join("python");
    let venv = python_dir.join(".venv");
    let python = if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    };
    let rust =
        |name: &'static str, bin: &str, port: u16, env: Vec<(&'static str, &'static str)>| Target {
            name,
            program: release.join(exe(bin)),
            args: vec![port.to_string()],
            env,
            port,
            artifact: release.join(exe(bin)),
        };
    vec![
        rust("sdkmcprust", "bench-sdkmcprust", 8101, vec![]),
        rust("rmcp", "bench-rmcp", 8102, vec![]),
        rust(
            "sdkmcprust (1 thread)",
            "bench-sdkmcprust",
            8104,
            vec![("TOKIO_WORKER_THREADS", "1")],
        ),
        rust(
            "rmcp (1 thread)",
            "bench-rmcp",
            8105,
            vec![("TOKIO_WORKER_THREADS", "1")],
        ),
        Target {
            name: "python mcp (1 process)",
            program: python,
            args: vec![
                python_dir.join("server.py").display().to_string(),
                "8103".to_owned(),
            ],
            env: vec![],
            port: 8103,
            artifact: venv,
        },
    ]
}

fn dir_size(path: &Path) -> u64 {
    if path.is_file() {
        return path.metadata().map_or(0, |m| m.len());
    }
    std::fs::read_dir(path).map_or(0, |entries| {
        entries.flatten().map(|e| dir_size(&e.path())).sum()
    })
}

/// Sends `server/discover` until it succeeds.
async fn wait_ready(authority: &str, timeout: Duration) -> Option<Duration> {
    let prepared = Prepared::new(&Scenario {
        name: "discover",
        method: "server/discover",
        tool: None,
        arguments: Value::Null,
    });
    let start = Instant::now();
    while start.elapsed() < timeout {
        // Short attempts: on Windows a refused loopback connection takes
        // about half a second to fail, which would dominate the measurement.
        let connect = tokio::time::timeout(
            Duration::from_millis(20),
            tokio::net::TcpStream::connect(authority),
        );
        if let Ok(Ok(stream)) = connect.await
            && let Ok((mut sender, connection)) =
                hyper::client::conn::http1::handshake(TokioIo::new(stream)).await
        {
            tokio::spawn(connection);
            if let Ok(response) = sender.send_request(prepared.request(authority)).await
                && response.status().is_success()
            {
                return Some(start.elapsed());
            }
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    None
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn machine() -> Value {
    let mut system = System::new();
    system.refresh_cpu_all();
    system.refresh_memory();
    json!({
        "cpu": system.cpus().first().map(|c| c.brand().trim().to_owned()),
        "logical_cores": system.cpus().len(),
        "memory_gib": (system.total_memory() as f64 / 1024f64.powi(3)).round(),
        "os": System::long_os_version(),
    })
}

async fn suite(quick: bool, out: &Path) -> Result<(), Error> {
    let root = std::env::current_dir()?;
    let (concurrency, warmup, duration): (&[usize], _, _) = if quick {
        (&[1, 64], Duration::from_secs(1), Duration::from_secs(3))
    } else {
        (
            &[1, 16, 64, 256],
            Duration::from_secs(2),
            Duration::from_secs(8),
        )
    };
    let mut infos = Vec::new();
    let mut results = Vec::new();
    for target in targets(&root) {
        if !target.program.exists() {
            eprintln!(
                "skipping {}: {} not found",
                target.name,
                target.program.display()
            );
            continue;
        }
        eprintln!("== {}", target.name);
        let authority = format!("127.0.0.1:{}", target.port);
        let child = Command::new(&target.program)
            .args(&target.args)
            .envs(target.env.iter().copied())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let pid = child.id();
        let child = Running(child);
        let startup = wait_ready(&authority, Duration::from_secs(60)).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let idle = Probe::new(Some(pid)).sample().map(|(rss, _)| rss);
        infos.push(ServerInfo {
            server: target.name.to_owned(),
            startup_ms: startup.map(|d| d.as_secs_f64() * 1000.0),
            idle_rss_mib: idle.map(|b| b as f64 / (1024.0 * 1024.0)),
            artifact_mib: Some(dir_size(&target.artifact) as f64 / (1024.0 * 1024.0)),
            artifact: target
                .artifact
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        });
        if startup.is_none() {
            eprintln!("   did not start; skipped");
            continue;
        }
        for scenario in scenarios() {
            for &c in concurrency {
                let m = measure(
                    target.name,
                    &authority,
                    Some(pid),
                    &scenario,
                    c,
                    warmup,
                    duration,
                )
                .await;
                eprintln!(
                    "   {:<16} c={:<4} {:>9.0} req/s  p50 {:>6} µs  p99 {:>7} µs  errors {}",
                    m.scenario, m.concurrency, m.rps, m.p50_us, m.p99_us, m.errors
                );
                results.push(m);
            }
        }
        drop(child);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    std::fs::create_dir_all(out)?;
    let report = json!({
        "date": std::env::var("BENCH_DATE").ok(),
        "machine": machine(),
        "quick": quick,
        "warmup_s": warmup.as_secs_f64(),
        "duration_s": duration.as_secs_f64(),
        "servers": infos,
        "results": results,
    });
    let path = out.join("results.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    eprintln!("wrote {}", path.display());
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("suite") => {
            let quick = args.iter().any(|a| a == "--quick");
            let out = args
                .iter()
                .position(|a| a == "--out")
                .and_then(|i| args.get(i + 1))
                .map_or_else(|| PathBuf::from("bench/results"), PathBuf::from);
            suite(quick, &out).await
        }
        Some("run") => {
            let url = args
                .get(1)
                .ok_or("missing authority, e.g. 127.0.0.1:8001")?;
            let name = args.get(2).ok_or("missing scenario")?;
            let concurrency: usize = args.get(3).map_or(Ok(16), |c| c.parse())?;
            let seconds: u64 = args.get(4).map_or(Ok(5), |s| s.parse())?;
            let scenario = scenarios()
                .into_iter()
                .find(|s| s.name == name)
                .ok_or("unknown scenario")?;
            let m = measure(
                "adhoc",
                url,
                None,
                &scenario,
                concurrency,
                Duration::from_secs(1),
                Duration::from_secs(seconds),
            )
            .await;
            println!("{}", serde_json::to_string_pretty(&m)?);
            Ok(())
        }
        _ => {
            eprintln!(
                "usage: mcp-load suite [--quick] [--out DIR] | mcp-load run AUTHORITY SCENARIO [CONCURRENCY] [SECONDS]"
            );
            Ok(())
        }
    }
}
