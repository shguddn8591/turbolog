//! TurboLog — Ultralight time-series log vector engine.
//!
//! Index thousands of log streams per second in real-time and detect anomalies
//! without high-cost GPUs or heavy vector databases. Data flow:
//! Ingest → Parse(Drain) → Embed(Cache/ONNX) → Tier 1/2 Detection → Ping-Pong Indexing → Flush

#[cfg(feature = "server")]
pub mod chunks;
pub mod cli;
pub mod detect;
pub mod embedded;
#[cfg(feature = "server")]
pub mod engine;
pub mod history;
#[cfg(feature = "server")]
pub mod http;
#[cfg(feature = "server")]
pub mod index;
pub mod ingest;
pub mod llm;
#[cfg(feature = "server")]
pub mod metrics;
pub mod pipeline;
pub mod scan;
#[cfg(feature = "server")]
pub mod wal;
pub mod watch;

#[cfg(feature = "tui")]
pub mod tui;

pub use detect::{AnomalyDetector, DetectionResult};
#[cfg(feature = "server")]
pub use engine::{EngineConfig, TurboLogEngine};
#[cfg(feature = "server")]
pub use index::PingPongIndexer;
pub use ingest::{Embedder, ParsedLog, TemplateParser, VectorCache};
#[cfg(feature = "server")]
pub use wal::Wal;
