//! HTTP Interface — Ingestion (/logs), search (/search), and stats (/stats).
//!
//! Implemented using `tiny_http` + a worker thread pool tailored for synchronous execution (no async runtime overhead).
//! gRPC support can be built on top of the same engine when required.

use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::engine::TurboLogEngine;

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

/// Spawns the server and returns the bound socket address and worker join handles.
/// Providing port `0` in `addr` lets the OS allocate an ephemeral port (ideal for testing).
pub fn run_server(
    engine: Arc<TurboLogEngine>,
    addr: &str,
    workers: usize,
) -> Result<(SocketAddr, Vec<JoinHandle<()>>)> {
    let server = Arc::new(Server::http(addr).map_err(|e| anyhow!("Failed to bind HTTP server: {e}"))?);
    let local = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow!("Not a valid IP address"))?;
    let handles = (0..workers.max(1))
        .map(|_| {
            let server = Arc::clone(&server);
            let engine = Arc::clone(&engine);
            std::thread::spawn(move || {
                while let Ok(request) = server.recv() {
                    handle(&engine, request);
                }
            })
        })
        .collect();
    Ok((local, handles))
}

fn handle(engine: &TurboLogEngine, mut request: Request) {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        respond(request, 400, json!({"error": "Failed to read request body"}));
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
