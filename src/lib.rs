//! TurboLog — 초경량 시계열 로그 벡터 엔진.
//!
//! GPU·무거운 벡터 DB 없이 초당 수천 건의 로그 스트림을 실시간 인덱싱하고
//! 이상 징후를 탐지한다. 데이터 흐름:
//! Ingest → Parse(Drain) → Embed(캐시/ONNX) → Tier 1/2 탐지 → Ping-Pong 인덱싱 → Flush

pub mod chunks;
pub mod detect;
pub mod engine;
pub mod http;
pub mod index;
pub mod ingest;
pub mod wal;

pub use detect::{AnomalyDetector, DetectionResult};
pub use engine::{EngineConfig, TurboLogEngine};
pub use index::PingPongIndexer;
pub use ingest::{Embedder, ParsedLog, TemplateParser, VectorCache};
pub use wal::Wal;
