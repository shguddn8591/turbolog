//! TurboLog — local-first log triage (CLI) with an optional experimental HTTP engine.
//!
//! Primary surface: pipe CLI (`watch` / `scan` / `history`) over Drain → MiniLM → novelty score.
//! Optional `--features server` stack: Ingest → Parse → Embed → Tier 1/2 → Ping-Pong index → WAL.
//! Product scope: `tasks/todo.md` (CLI-first; not a fleet observability platform).

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
