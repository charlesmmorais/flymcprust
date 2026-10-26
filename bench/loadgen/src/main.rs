//! `mcp-load`: closed-loop load generator for MCP servers (2026-07-28),
//! over Streamable HTTP (JSON responses) or stdio.
//!
//! ```text
//! mcp-load suite [--quick] [--repeat N] [--server-cpus LIST] [--only TEXT] [--out DIR]
//! mcp-load real  [--quick] [--repeat N] [--server-cpus LIST] [--only TEXT] [--out DIR]
//! mcp-load run AUTHORITY SCENARIO [CONCURRENCY] [SECONDS]
//! ```
//!
//! `real` runs the real-world suite (see `real.rs`).
//!
//! HTTP: each simulated client owns one keep-alive HTTP/1.1 connection and
//! sends a new request as soon as the previous response arrives. stdio:
//! each simulated client keeps one request in flight on the server's single
//! pipe. Latency is recorded in an HDR histogram after a warm-up period; the
//! server process's memory (RSS) and CPU time are sampled through `sysinfo`.
//!
//! `--server-cpus 0-1` (Linux) pins the server to those CPUs with `taskset`;
//! run the load generator itself on the others (`taskset -c 2-3 mcp-load
//! ...`) so the two do not compete. `--repeat N` measures every point N
//! times; the report shows the median and the spread.

mod real;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use hdrhistogram::Histogram;
use http_body_util::{BodyExt as _, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
use tokio::sync::{mpsc, oneshot};

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

/// The JSON-RPC message of a scenario, without its `id`.
fn message(s: &Scenario) -> Value {
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
    json!({ "jsonrpc": "2.0", "method": s.method, "params": params })
}

/// A prepared HTTP request: body and headers built once.
struct Prepared {
    body: Bytes,
    method: &'static str,
    tool: Option<&'static str>,
    bearer: Option<String>,
}

impl Prepared {
    fn new(s: &Scenario, token: Option<&str>) -> Self {
        let mut body = message(s);
        body["id"] = json!(1);
        Self {
            body: Bytes::from(serde_json::to_vec(&body).unwrap_or_default()),
            method: s.method,
            tool: s.tool,
            bearer: token.map(|t| format!("Bearer {t}")),
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
        if let Some(bearer) = &self.bearer {
            builder = builder.header("authorization", bearer);
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

impl WorkerStats {
    fn new() -> Self {
        Self {
            histogram: Histogram::new_with_bounds(1, 60_000_000, 3).ok(),
            ..Self::default()
        }
    }

    fn record(&mut self, started: Instant, record_from: Instant, ok: bool) {
        if started < record_from {
            return;
        }
        if ok {
            self.ok += 1;
            if let Some(h) = &mut self.histogram {
                let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
                let _ = h.record(micros.max(1));
            }
        } else {
            self.errors += 1;
        }
    }
}

/// One closed-loop HTTP client.
async fn http_worker(
    authority: Arc<str>,
    prepared: Arc<Prepared>,
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
            let ok = match sender.send_request(prepared.request(&authority)).await {
                Ok(response) => {
                    let ok = response.status().is_success();
                    match response.into_body().collect().await {
                        Ok(body) => ok && !body.to_bytes().is_empty(),
                        Err(_) => false,
                    }
                }
                Err(_) => false,
            };
            stats.record(started, record_from, ok);
            if !ok {
                continue 'connect;
            }
        }
    }
    stats
}

/// A server spoken to over its stdin and stdout. Responses are matched to
/// requests by id.
#[derive(Clone)]
struct StdioPeer {
    lines: mpsc::Sender<Vec<u8>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<bool>>>>,
    next_id: Arc<AtomicU64>,
}

#[derive(Deserialize)]
struct Reply {
    id: Option<u64>,
    result: Option<serde::de::IgnoredAny>,
}

impl StdioPeer {
    fn start(child: &mut Child) -> Option<Self> {
        let stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        let mut stdin = tokio::process::ChildStdin::from_std(stdin).ok()?;
        let stdout = tokio::process::ChildStdout::from_std(stdout).ok()?;
        let (lines, mut outgoing) = mpsc::channel::<Vec<u8>>(4096);
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<bool>>>> = Arc::default();
        tokio::spawn(async move {
            while let Some(line) = outgoing.recv().await {
                if stdin.write_all(&line).await.is_err() {
                    break;
                }
            }
        });
        let replies = Arc::clone(&pending);
        tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(stdout);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let Ok(reply) = serde_json::from_slice::<Reply>(&line) else {
                    continue;
                };
                let Some(id) = reply.id else { continue };
                let waiter = replies.lock().ok().and_then(|mut p| p.remove(&id));
                if let Some(waiter) = waiter {
                    let _ = waiter.send(reply.result.is_some());
                }
            }
        });
        Some(Self {
            lines,
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
        })
    }

    /// Sends one request (`template` is the message without its leading
    /// `{`) and waits for the answer.
    async fn call(&self, template: &str) -> bool {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(id, tx);
        }
        let line = format!("{{\"id\":{id},{template}\n").into_bytes();
        if self.lines.send(line).await.is_err() {
            return false;
        }
        matches!(
            tokio::time::timeout(Duration::from_secs(30), rx).await,
            Ok(Ok(true))
        )
    }
}

/// The message of a scenario as a stdio line template.
fn stdio_template(s: &Scenario) -> String {
    let text = message(s).to_string();
    text.strip_prefix('{').unwrap_or(&text).to_owned()
}

/// One closed-loop stdio client.
async fn stdio_worker(
    peer: StdioPeer,
    template: Arc<str>,
    record_from: Instant,
    until: Instant,
) -> WorkerStats {
    let mut stats = WorkerStats::new();
    while Instant::now() < until {
        let started = Instant::now();
        let ok = peer.call(&template).await;
        stats.record(started, record_from, ok);
        if !ok {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    stats
}

#[derive(Serialize, Clone)]
struct Measurement {
    server: String,
    transport: &'static str,
    scenario: String,
    concurrency: usize,
    /// Repetition index (0-based).
    run: usize,
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
    /// the real interpreter as a child process. Threads, which `sysinfo`
    /// lists as processes on Linux, are skipped: they share the process's
    /// memory and their CPU time is already in the process's total.
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
                    .filter(|(_, p)| p.parent() == Some(parent) && p.thread_kind().is_none())
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

/// How the load reaches the server.
enum Driver {
    Http {
        authority: Arc<str>,
        token: Option<String>,
    },
    Stdio(StdioPeer),
}

#[allow(clippy::too_many_arguments)]
async fn measure(
    server: &str,
    driver: &Driver,
    pid: Option<u32>,
    scenario: &Scenario,
    concurrency: usize,
    run: usize,
    warmup: Duration,
    duration: Duration,
) -> Measurement {
    let start = Instant::now();
    let record_from = start + warmup;
    let until = record_from + duration;
    let (handles, transport): (Vec<_>, _) = match driver {
        Driver::Http { authority, token } => {
            let prepared = Arc::new(Prepared::new(scenario, token.as_deref()));
            let handles = (0..concurrency)
                .map(|_| {
                    tokio::spawn(http_worker(
                        Arc::clone(authority),
                        Arc::clone(&prepared),
                        record_from,
                        until,
                    ))
                })
                .collect();
            (handles, "http")
        }
        Driver::Stdio(peer) => {
            let template: Arc<str> = Arc::from(stdio_template(scenario));
            let handles = (0..concurrency)
                .map(|_| {
                    tokio::spawn(stdio_worker(
                        peer.clone(),
                        Arc::clone(&template),
                        record_from,
                        until,
                    ))
                })
                .collect();
            (handles, "stdio")
        }
    };

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
        transport,
        scenario: scenario.name.to_owned(),
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
            // No progress means the sampler missed the window (seen on
            // Windows): report nothing rather than zero.
            (Some(a), Some(b)) if ok > 0 && b > a => Some((b - a) as f64 * 1000.0 / ok as f64),
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
    /// `None`: stdio.
    port: Option<u16>,
    /// Bearer token sent with every request (OAuth).
    token: Option<String>,
    /// What is measured as the deployable artifact.
    artifact: PathBuf,
}

#[derive(Serialize)]
struct ServerInfo {
    server: String,
    transport: &'static str,
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
    let fixtures = root.join("bench").join("fixtures");
    let python_dir = root.join("bench").join("servers").join("python");
    let venv = python_dir.join(".venv");
    let python = if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    };
    let server_py = python_dir.join("server.py").display().to_string();
    let rust = |name: &'static str,
                bin: &str,
                port: Option<u16>,
                env: Vec<(&'static str, &'static str)>| Target {
        name,
        program: release.join(exe(bin)),
        args: vec![port.map_or_else(|| "stdio".to_owned(), |p| p.to_string())],
        env,
        port,
        token: None,
        artifact: release.join(exe(bin)),
    };
    let token = std::fs::read_to_string(fixtures.join("token.txt"))
        .ok()
        .map(|t| t.trim().to_owned());
    let mut oauth = rust(
        "flymcprust + OAuth (JWT RS256)",
        "bench-flymcprust",
        Some(8106),
        vec![],
    );
    oauth.args.push("oauth".to_owned());
    oauth
        .args
        .push(fixtures.join("jwks.json").display().to_string());
    oauth.token = token;
    vec![
        rust("flymcprust", "bench-flymcprust", Some(8101), vec![]),
        rust("rmcp", "bench-rmcp", Some(8102), vec![]),
        rust(
            "flymcprust (1 thread)",
            "bench-flymcprust",
            Some(8104),
            vec![("TOKIO_WORKER_THREADS", "1")],
        ),
        rust(
            "rmcp (1 thread)",
            "bench-rmcp",
            Some(8105),
            vec![("TOKIO_WORKER_THREADS", "1")],
        ),
        Target {
            name: "python mcp (1 process)",
            program: python.clone(),
            args: vec![server_py.clone(), "8103".to_owned()],
            env: vec![],
            port: Some(8103),
            token: None,
            artifact: venv.clone(),
        },
        oauth,
        rust("flymcprust", "bench-flymcprust", None, vec![]),
        rust("rmcp", "bench-rmcp", None, vec![]),
        Target {
            name: "python mcp (1 process)",
            program: python,
            args: vec![server_py, "stdio".to_owned()],
            env: vec![],
            port: None,
            token: None,
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

fn discover() -> Scenario {
    Scenario {
        name: "discover",
        method: "server/discover",
        tool: None,
        arguments: Value::Null,
    }
}

/// Sends `server/discover` until it succeeds.
async fn wait_ready(driver: &Driver, timeout: Duration) -> Option<Duration> {
    let start = Instant::now();
    match driver {
        Driver::Stdio(peer) => {
            let template = stdio_template(&discover());
            let ready = tokio::time::timeout(timeout, peer.call(&template)).await;
            matches!(ready, Ok(true)).then(|| start.elapsed())
        }
        Driver::Http { authority, token } => {
            let prepared = Prepared::new(&discover(), token.as_deref());
            while start.elapsed() < timeout {
                // Short attempts: on Windows a refused loopback connection
                // takes about half a second to fail, which would dominate the
                // measurement.
                let connect = tokio::time::timeout(
                    Duration::from_millis(20),
                    tokio::net::TcpStream::connect(&**authority),
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
    }
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn machine(server_cpus: Option<&str>) -> Value {
    let mut system = System::new();
    system.refresh_cpu_all();
    system.refresh_memory();
    json!({
        "cpu": system.cpus().first().map(|c| c.brand().trim().to_owned()),
        "logical_cores": system.cpus().len(),
        "memory_gib": (system.total_memory() as f64 / 1024f64.powi(3)).round(),
        "os": System::long_os_version(),
        "arch": std::env::consts::ARCH,
        "server_cpus": server_cpus,
    })
}

struct Options {
    quick: bool,
    /// Only targets whose name or transport contains this text.
    only: Option<String>,
    repeat: usize,
    server_cpus: Option<String>,
    out: PathBuf,
}

async fn suite(options: &Options) -> Result<(), Error> {
    let root = std::env::current_dir()?;
    let (http_levels, stdio_levels, warmup, duration): (&[usize], &[usize], _, _) = if options.quick
    {
        (
            &[1, 64],
            &[1, 16],
            Duration::from_secs(1),
            Duration::from_secs(3),
        )
    } else {
        (
            &[1, 16, 64, 256, 1024],
            &[1, 16, 64],
            Duration::from_secs(2),
            Duration::from_secs(8),
        )
    };
    let mut infos = Vec::new();
    let mut results = Vec::new();
    for target in targets(&root) {
        let transport = if target.port.is_some() {
            "http"
        } else {
            "stdio"
        };
        if let Some(only) = &options.only
            && !format!("{} {transport}", target.name).contains(only.as_str())
        {
            continue;
        }
        if !target.program.exists() {
            eprintln!(
                "skipping {} ({transport}): {} not found",
                target.name,
                target.program.display()
            );
            continue;
        }
        eprintln!("== {} ({transport})", target.name);
        let mut command = match &options.server_cpus {
            Some(cpus) if cfg!(target_os = "linux") => {
                let mut c = Command::new("taskset");
                c.arg("-c").arg(cpus).arg(&target.program);
                c
            }
            _ => Command::new(&target.program),
        };
        command
            .args(&target.args)
            .envs(target.env.iter().copied())
            .stderr(Stdio::null());
        if target.port.is_some() {
            command.stdin(Stdio::null()).stdout(Stdio::null());
        } else {
            command.stdin(Stdio::piped()).stdout(Stdio::piped());
        }
        let mut child = command.spawn()?;
        let pid = child.id();
        let driver = match target.port {
            Some(port) => Driver::Http {
                authority: Arc::from(format!("127.0.0.1:{port}")),
                token: target.token.clone(),
            },
            None => match StdioPeer::start(&mut child) {
                Some(peer) => Driver::Stdio(peer),
                None => continue,
            },
        };
        let child = Running(child);
        let startup = wait_ready(&driver, Duration::from_secs(60)).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let idle = Probe::new(Some(pid)).sample().map(|(rss, _)| rss);
        infos.push(ServerInfo {
            server: target.name.to_owned(),
            transport,
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
            eprintln!("   did not answer server/discover; skipped");
            continue;
        }
        let levels = if target.port.is_some() {
            http_levels
        } else {
            stdio_levels
        };
        for scenario in scenarios() {
            for &c in levels {
                for run in 0..options.repeat {
                    let m = measure(
                        target.name,
                        &driver,
                        Some(pid),
                        &scenario,
                        c,
                        run,
                        warmup,
                        duration,
                    )
                    .await;
                    eprintln!(
                        "   {:<16} c={:<4} #{run} {:>9.0} req/s  p50 {:>6} µs  p99 {:>7} µs  errors {}",
                        m.scenario, m.concurrency, m.rps, m.p50_us, m.p99_us, m.errors
                    );
                    results.push(m);
                }
            }
        }
        drop(driver);
        drop(child);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    std::fs::create_dir_all(&options.out)?;
    let python_version = targets(&root)
        .into_iter()
        .find(|t| t.name.starts_with("python"))
        .and_then(|t| Command::new(t.program).arg("--version").output().ok())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    let report = json!({
        "date": std::env::var("BENCH_DATE").ok(),
        "commit": std::env::var("BENCH_COMMIT").ok(),
        "python": python_version,
        "machine": machine(options.server_cpus.as_deref()),
        "quick": options.quick,
        "repeat": options.repeat,
        "warmup_s": warmup.as_secs_f64(),
        "duration_s": duration.as_secs_f64(),
        "servers": infos,
        "results": results,
    });
    let path = options.out.join("results.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    eprintln!("wrote {}", path.display());
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    match args.first().map(String::as_str) {
        Some("suite") => {
            let options = Options {
                quick: args.iter().any(|a| a == "--quick"),
                only: value("--only"),
                repeat: value("--repeat").map_or(Ok(1), |r| r.parse())?.max(1),
                server_cpus: value("--server-cpus"),
                out: value("--out").map_or_else(|| PathBuf::from("bench/results"), PathBuf::from),
            };
            suite(&options).await
        }
        Some("real") => {
            let options = Options {
                quick: args.iter().any(|a| a == "--quick"),
                only: value("--only"),
                repeat: value("--repeat").map_or(Ok(1), |r| r.parse())?.max(1),
                server_cpus: value("--server-cpus"),
                out: value("--out")
                    .map_or_else(|| PathBuf::from("bench/results/real"), PathBuf::from),
            };
            real::suite(&options).await
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
            let driver = Driver::Http {
                authority: Arc::from(url.as_str()),
                token: std::env::var("MCP_LOAD_TOKEN").ok(),
            };
            let m = measure(
                "adhoc",
                &driver,
                None,
                &scenario,
                concurrency,
                0,
                Duration::from_secs(1),
                Duration::from_secs(seconds),
            )
            .await;
            println!("{}", serde_json::to_string_pretty(&m)?);
            Ok(())
        }
        _ => {
            eprintln!(
                "usage: mcp-load suite [--quick] [--repeat N] [--server-cpus LIST] [--out DIR] | \
                 mcp-load run AUTHORITY SCENARIO [CONCURRENCY] [SECONDS]"
            );
            Ok(())
        }
    }
}
