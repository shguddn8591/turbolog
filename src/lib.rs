//! TurboLog — Ultralight time-series log vector engine.
//!
//! Index thousands of log streams per second in real-time and detect anomalies
//! without high-cost GPUs or heavy vector databases. Data flow:
//! Ingest → Parse(Drain) → Embed(Cache/ONNX) → Tier 1/2 Detection → Ping-Pong Indexing → Flush

pub mod chunks;
pub mod cli;
pub mod detect;
pub mod embedded;
pub mod engine;
pub mod http;
pub mod index;
pub mod ingest;
pub mod metrics;
pub mod pipeline;
pub mod scan;
pub mod wal;
pub mod watch;

#[cfg(feature = "tui")]
pub mod tui;

pub use detect::{AnomalyDetector, DetectionResult};
pub use engine::{EngineConfig, TurboLogEngine};
pub use index::PingPongIndexer;
pub use ingest::{Embedder, ParsedLog, TemplateParser, VectorCache};
pub use wal::Wal;
