//! HTTP 인터페이스 — 삽입(/logs)·검색(/search)·상태(/stats).
//!
//! 동기식 엔진에 맞춰 `tiny_http` + 워커 스레드 풀로 구현 (async 런타임 불필요).
//! gRPC는 필요해질 때 같은 엔진 위에 추가한다.

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

/// 서버를 띄우고 (바인딩된 주소, 워커 핸들들)을 반환한다. `addr`에 포트 0을 주면
/// 임시 포트가 배정된다 (테스트용).
pub fn run_server(
    engine: Arc<TurboLogEngine>,
    addr: &str,
    workers: usize,
) -> Result<(SocketAddr, Vec<JoinHandle<()>>)> {
    let server = Arc::new(Server::http(addr).map_err(|e| anyhow!("HTTP 바인딩 실패: {e}"))?);
    let local = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow!("IP 주소 아님"))?;
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
        respond(request, 400, json!({"error": "본문 읽기 실패"}));
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
        Err(e) => return (400, json!({"error": format!("잘못된 요청: {e}")})),
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
        Err(e) => return (400, json!({"error": format!("잘못된 요청: {e}")})),
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
