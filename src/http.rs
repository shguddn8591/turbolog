//! HTTP Interface — Ingestion (/logs), search (/search), stats (/stats),
//! health (/health), readiness (/ready), and metrics (/metrics).
//!
//! Implemented using `tiny_http` + a worker thread pool tailored for synchronous execution (no async runtime overhead).
//! gRPC support can be built on top of the same engine when required.

use std::io::Read;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::engine::TurboLogEngine;
use crate::metrics;

#[derive(Deserialize)]
struct LogsBody {
    logs: Vec<String>,
}

#[derive(Deserialize)]
struct SearchBody {
    query: String,
    #[serde(default = "default_k")]
    k: usize,
}

fn default_k() -> usize {
    5
}

/// Request body size limit — protects memory from oversized payloads (HTTP 413 beyond this).
/// 1 MiB ≈ several thousand typical log lines per batch.
const MAX_BODY_BYTES: u64 = 1024 * 1024;

/// Configuration for the HTTP server.
pub struct ServerConfig {
    pub addr: String,
    pub workers: usize,
    pub auth_token: Option<String>,
    /// Maximum concurrent in-flight requests. 0 = unlimited.
    pub max_inflight: usize,
    /// Set to `true` to signal workers to stop polling and exit gracefully.
    pub shutdown: Arc<AtomicBool>,
}

/// Full-featured server entry point. Workers poll `cfg.shutdown` every 200 ms so
/// `SIGTERM`/`SIGINT` can drain cleanly without killing mid-request.
///
/// Providing port `0` in `cfg.addr` lets the OS allocate an ephemeral port (ideal for testing).
pub fn run_server_with(
    engine: Arc<TurboLogEngine>,
    cfg: ServerConfig,
) -> Result<(SocketAddr, Vec<JoinHandle<()>>)> {
    let server = Arc::new(
        Server::http(&cfg.addr).map_err(|e| anyhow!("Failed to bind HTTP server: {e}"))?,
    );
    let local = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow!("Not a valid IP address"))?;

    let auth_token = Arc::new(cfg.auth_token);
    let shutdown = Arc::clone(&cfg.shutdown);
    let inflight = Arc::new(AtomicUsize::new(0));
    let max_inflight = cfg.max_inflight;

    let handles = (0..cfg.workers.max(1))
        .map(|_| {
            let server = Arc::clone(&server);
            let engine = Arc::clone(&engine);
            let auth_token = Arc::clone(&auth_token);
            let shutdown = Arc::clone(&shutdown);
            let inflight = Arc::clone(&inflight);
            std::thread::spawn(move || loop {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                match server.recv_timeout(Duration::from_millis(200)) {
                    Ok(Some(request)) => {
                        handle(&engine, auth_token.as_deref(), max_inflight, &inflight, request);
                    }
                    Ok(None) => {
                        // timeout — check shutdown flag on next iteration
                    }
                    Err(_) => break,
                }
            })
        })
        .collect();

    Ok((local, handles))
}

/// Backward-compatible 4-arg wrapper. Delegates to `run_server_with` with no backpressure
/// limit and a fresh (never-set) shutdown flag. Existing tests and examples compile unchanged.
pub fn run_server(
    engine: Arc<TurboLogEngine>,
    addr: &str,
    workers: usize,
    auth_token: Option<String>,
) -> Result<(SocketAddr, Vec<JoinHandle<()>>)> {
    run_server_with(
        engine,
        ServerConfig {
            addr: addr.to_owned(),
            workers,
            auth_token,
            max_inflight: 0,
            shutdown: Arc::new(AtomicBool::new(false)),
        },
    )
}

fn authorized(request: &Request, token: &str) -> bool {
    let expected = format!("Bearer {token}");
    request
        .headers()
        .iter()
        .any(|h| h.field.equiv("Authorization") && h.value.as_str() == expected)
}

/// Returns `true` for GET operational-probe routes that are exempt from auth and body reading.
fn is_probe(method: &Method, url: &str) -> bool {
    matches!(method, Method::Get) && matches!(url, "/health" | "/ready" | "/metrics")
}

fn handle(
    engine: &TurboLogEngine,
    auth_token: Option<&str>,
    max_inflight: usize,
    inflight: &AtomicUsize,
    mut request: Request,
) {
    // Operational probes bypass auth and body limits entirely.
    if is_probe(request.method(), request.url()) {
        let url = request.url().to_owned();
        match url.as_str() {
            "/health" => {
                metrics::inc_http(200);
                respond(request, 200, json!({"status": "ok"}));
            }
            "/ready" => {
                let s = engine.stats();
                metrics::inc_http(200);
                respond(
                    request,
                    200,
                    json!({
                        "status": "ok",
                        "detector_calibrated": s.detector_calibrated,
                        "pending_window_len": s.pending_window_len,
                        "ring_windows": s.ring_windows,
                        "ingested_total": s.ingested_total,
                    }),
                );
            }
            "/metrics" => {
                let s = engine.stats();
                let extra: &[(&str, &str, f64)] = &[
                    ("turbolog_cache_hit_rate", "Embedding cache hit rate", s.cache_hit_rate),
                    (
                        "turbolog_pending_window",
                        "Logs in current unsealed window",
                        s.pending_window_len as f64,
                    ),
                    (
                        "turbolog_ring_vectors",
                        "Total vectors in sealed ring windows",
                        s.ring_vectors as f64,
                    ),
                    (
                        "turbolog_detector_calibrated",
                        "1 if anomaly detector is calibrated",
                        if s.detector_calibrated { 1.0 } else { 0.0 },
                    ),
                ];
                let text = metrics::render(extra);
                let ct = Header::from_bytes(
                    &b"Content-Type"[..],
                    &b"text/plain; version=0.0.4"[..],
                )
                .unwrap();
                let resp = Response::from_string(text)
                    .with_status_code(200)
                    .with_header(ct);
                metrics::inc_http(200);
                let _ = request.respond(resp);
            }
            _ => unreachable!(),
        }
        return;
    }

    // Auth check for non-probe routes.
    if let Some(token) = auth_token {
        if !authorized(&request, token) {
            metrics::inc_http(401);
            respond(request, 401, json!({"error": "Unauthorized"}));
            return;
        }
    }

    // Backpressure: shed request if we are at capacity.
    if max_inflight > 0 {
        let prev = inflight.fetch_add(1, Ordering::Relaxed);
        if prev >= max_inflight {
            inflight.fetch_sub(1, Ordering::Relaxed);
            metrics::inc_http_rejected();
            metrics::inc_http(503);
            respond(request, 503, json!({"error": "overloaded"}));
            return;
        }
        metrics::inflight_inc();
    }

    // Body read with size guard.
    let mut body = String::new();
    // take(limit + 1): reading one byte past the limit detects oversized bodies
    // without ever buffering more than MAX_BODY_BYTES + 1.
    // (`Read::take` UFCS form — `&mut dyn Read` is Sized, the trait object itself is not.)
    if std::io::Read::take(request.as_reader(), MAX_BODY_BYTES + 1)
        .read_to_string(&mut body)
        .is_err()
    {
        if max_inflight > 0 {
            inflight.fetch_sub(1, Ordering::Relaxed);
            metrics::inflight_dec();
        }
        metrics::inc_http(400);
        respond(request, 400, json!({"error": "Failed to read request body"}));
        return;
    }
    if body.len() as u64 > MAX_BODY_BYTES {
        if max_inflight > 0 {
            inflight.fetch_sub(1, Ordering::Relaxed);
            metrics::inflight_dec();
        }
        metrics::inc_http(413);
        respond(
            request,
            413,
            json!({"error": format!("Request body exceeds {MAX_BODY_BYTES} bytes")}),
        );
        return;
    }

    let (status, payload) = match (request.method(), request.url()) {
        (Method::Post, "/logs") => post_logs(engine, &body),
        (Method::Post, "/search") => post_search(engine, &body),
        (Method::Get, "/stats") => match serde_json::to_value(engine.stats()) {
            Ok(v) => (200, v),
            Err(e) => (500, json!({"error": e.to_string()})),
        },
        _ => (404, json!({"error": "not found"})),
    };

    if max_inflight > 0 {
        inflight.fetch_sub(1, Ordering::Relaxed);
        metrics::inflight_dec();
    }
    metrics::inc_http(status as u16);
    respond(request, status, payload);
}

fn post_logs(engine: &TurboLogEngine, body: &str) -> (u32, serde_json::Value) {
    let parsed: LogsBody = match serde_json::from_str(body) {
        Ok(p) => p,
        Err(e) => return (400, json!({"error": format!("Invalid request: {e}")})),
    };
    let mut results = Vec::with_capacity(parsed.logs.len());
    for line in &parsed.logs {
        match engine.ingest_log(line) {
            Ok(report) => results.push(serde_json::to_value(report).unwrap_or_default()),
            Err(e) => return (500, json!({"error": e.to_string()})),
        }
    }
    (200, json!({ "results": results }))
}

fn post_search(engine: &TurboLogEngine, body: &str) -> (u32, serde_json::Value) {
    let parsed: SearchBody = match serde_json::from_str(body) {
        Ok(p) => p,
        Err(e) => return (400, json!({"error": format!("Invalid request: {e}")})),
    };
    match engine.search_text(&parsed.query, parsed.k) {
        Ok(hits) => (
            200,
            json!({ "results": serde_json::to_value(hits).unwrap_or_default() }),
        ),
        Err(e) => (400, json!({"error": e.to_string()})),
    }
}

fn respond(request: Request, status: u32, payload: serde_json::Value) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let response = Response::from_string(payload.to_string())
        .with_status_code(status as u16)
        .with_header(header);
    let _ = request.respond(response);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineConfig, TurboLogEngine};

    fn test_engine() -> Arc<TurboLogEngine> {
        use crate::Embedder;
        let tmpdir =
            std::env::temp_dir().join(format!("turbolog_http_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmpdir).unwrap();
        let cfg = EngineConfig {
            data_dir: tmpdir,
            ..EngineConfig::default()
        };
        // Locate models relative to the workspace root (CARGO_MANIFEST_DIR).
        let model_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models");
        let embedder = Embedder::new(model_dir.join("model.onnx"), model_dir.join("tokenizer.json"))
            .expect("test embedder: model files must exist at models/");
        Arc::new(TurboLogEngine::open(cfg, vec![embedder]).unwrap())
    }

    fn start(engine: Arc<TurboLogEngine>, max_inflight: usize) -> (SocketAddr, Arc<AtomicBool>) {
        let shutdown = Arc::new(AtomicBool::new(false));
        let (addr, _handles) = run_server_with(
            engine,
            ServerConfig {
                addr: "127.0.0.1:0".to_owned(),
                workers: 2,
                auth_token: None,
                max_inflight,
                shutdown: Arc::clone(&shutdown),
            },
        )
        .unwrap();
        (addr, shutdown)
    }

    #[test]
    fn health_returns_200() {
        let (addr, shutdown) = start(test_engine(), 0);
        let resp = ureq::get(&format!("http://{addr}/health")).call().unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.into_json().unwrap();
        assert_eq!(body["status"], "ok");
        shutdown.store(true, Ordering::Relaxed);
    }

    #[test]
    fn metrics_contains_turbolog_prefix() {
        // Auth token set — /metrics must still be reachable without a Bearer header.
        let engine = test_engine();
        let shutdown = Arc::new(AtomicBool::new(false));
        let (addr, _handles) = run_server_with(
            engine,
            ServerConfig {
                addr: "127.0.0.1:0".to_owned(),
                workers: 1,
                auth_token: Some("secret".to_owned()),
                max_inflight: 0,
                shutdown: Arc::clone(&shutdown),
            },
        )
        .unwrap();

        let resp = ureq::get(&format!("http://{addr}/metrics")).call().unwrap();
        assert_eq!(resp.status(), 200);
        let text = resp.into_string().unwrap();
        assert!(text.contains("turbolog_"), "expected turbolog_ metrics, got:\n{text}");
        assert!(text.contains("turbolog_cache_hit_rate"), "missing cache_hit_rate gauge");

        shutdown.store(true, Ordering::Relaxed);
    }

    #[test]
    fn backpressure_returns_503() {
        // max_inflight=1 with 2 workers and 10 concurrent GET /stats requests.
        // At least one should be shed when the single slot is occupied.
        let (addr, shutdown) = start(test_engine(), 1);
        let url = format!("http://{addr}/stats");

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let u = url.clone();
                std::thread::spawn(move || {
                    ureq::get(&u).call().map(|r| r.status()).unwrap_or(503)
                })
            })
            .collect();

        let got_503 = handles.into_iter().any(|h| h.join().unwrap() == 503);
        assert!(got_503, "expected at least one 503 under max_inflight=1 with 10 concurrent requests");

        shutdown.store(true, Ordering::Relaxed);
    }
}
