//! TurboLog 서버 데몬.
//!
//! 환경 변수: TURBOLOG_PORT(8087), TURBOLOG_DATA_DIR(./data), TURBOLOG_MODEL_DIR(./models)

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use turbolog::engine::{EngineConfig, TurboLogEngine};
use turbolog::http::run_server;
use turbolog::Embedder;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn main() -> anyhow::Result<()> {
    let port = env_or("TURBOLOG_PORT", "8087");
    let model_dir = PathBuf::from(env_or("TURBOLOG_MODEL_DIR", "./models"));
    let cfg = EngineConfig {
        data_dir: PathBuf::from(env_or("TURBOLOG_DATA_DIR", "./data")),
        ..EngineConfig::default()
    };

    let embedder = Embedder::new(
        model_dir.join("model.onnx"),
        model_dir.join("tokenizer.json"),
    )?;
    let engine = Arc::new(TurboLogEngine::open(cfg, embedder)?);

    // 스왑 데몬: 10초 주기 봉인 + 1시간마다 보존 만료 청크 unlink
    {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || {
            let interval = Duration::from_secs(engine.config().swap_interval_secs);
            let mut last_sweep_hour = 0i64;
            loop {
                std::thread::sleep(interval);
                if let Err(e) = engine.swap_tick() {
                    eprintln!("swap_tick 실패: {e:#}");
                }
                let hour = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64 / 3600)
                    .unwrap_or(0);
                if hour != last_sweep_hour {
                    last_sweep_hour = hour;
                    match engine.sweep_chunks() {
                        Ok(0) => {}
                        Ok(n) => println!("보존 만료 청크 {n}개 삭제"),
                        Err(e) => eprintln!("sweep 실패: {e:#}"),
                    }
                }
            }
        });
    }

    let (addr, handles) = run_server(engine, &format!("0.0.0.0:{port}"), 4)?;
    println!("TurboLog listening on http://{addr}");
    for handle in handles {
        let _ = handle.join();
    }
    Ok(())
}
