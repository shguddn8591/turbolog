//! HTTP Interface — Ingestion (/logs), search (/search), and stats (/stats).
//!
//! Implemented using `tiny_http` + a worker thread pool tailored for synchronous execution (no async runtime overhead).
//! gRPC support can be built on top of the same engine when required.

use std::io::Read;
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

/// Request body size limit — protects memory from oversized payloads (HTTP 413 beyond this).
/// 1 MiB ≈ several thousand typical log lines per batch.
const MAX_BODY_BYTES: u64 = 1024 * 1024;

/// Spawns the server and returns the bound socket address and worker join handles.
/// Providing port `0` in `addr` lets the OS allocate an ephemeral port (ideal for testing).
///
/// `auth_token`: when `Some`, every request must carry `Authorization: Bearer <token>`
/// or it is rejected with HTTP 401. `None` disables auth (trusted-network deployments).
pub fn run_server(
    engine: Arc<TurboLogEngine>,
    addr: &str,
    workers: usize,
    auth_token: Option<String>,
) -> Result<(SocketAddr, Vec<JoinHandle<()>>)> {
    let server = Arc::new(Server::http(addr).map_err(|e| anyhow!("Failed to bind HTTP server: {e}"))?);
    let local = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow!("Not a valid IP address"))?;
    let auth_token = Arc::new(auth_token);
    let handles = (0..workers.max(1))
        .map(|_| {
            let server = Arc::clone(&server);
            let engine = Arc::clone(&engine);
            let auth_token = Arc::clone(&auth_token);
            std::thread::spawn(move || {
                while let Ok(request) = server.recv() {
                    handle(&engine, auth_token.as_deref(), request);
                }
            })
        })
        .collect();
    Ok((local, handles))
}

fn authorized(request: &Request, token: &str) -> bool {
    let expected = format!("Bearer {token}");
    request
        .headers()
        .iter()
        .any(|h| h.field.equiv("Authorization") && h.value.as_str() == expected)
}

fn handle(engine: &TurboLogEngine, auth_token: Option<&str>, mut request: Request) {
    if let Some(token) = auth_token {
        if !authorized(&request, token) {
            respond(request, 401, json!({"error": "Unauthorized"}));
            return;
        }
    }

    let mut body = String::new();
    // take(limit + 1): reading one byte past the limit detects oversized bodies
    // without ever buffering more than MAX_BODY_BYTES + 1.
    // (`Read::take` UFCS form — `&mut dyn Read` is Sized, the trait object itself is not.)
    if std::io::Read::take(request.as_reader(), MAX_BODY_BYTES + 1)
        .read_to_string(&mut body)
        .is_err()
    {
        respond(request, 400, json!({"error": "Failed to read request body"}));
        return;
    }
    if body.len() as u64 > MAX_BODY_BYTES {
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
