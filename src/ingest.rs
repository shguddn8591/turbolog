//! Ingestion & Cache Layer — Gateway for transforming raw text logs into vectors.
//!
//! Cache hits bypass embedding computation completely (cost: 0). Cache misses trigger CPU (ONNX) model inference.
//!
//! System Constraint (Spec v1.0 §4.3 — Stateless Embedder):
//! The `Embedder` must remain stateless across requests to allow horizontal scaling on separate thread pools.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use drain_rs::DrainTree;
use lru::LruCache;
use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;

/// Structured log after Drain parsing.
pub struct ParsedLog {
    pub template_id: u64,
    /// Static template string where dynamic variables are replaced by `<*>` (used as embedding input).
    pub template: String,
    /// Ingestion timestamp (Unix epoch in seconds).
    pub timestamp: i64,
    pub metadata: HashMap<String, String>,
    pub raw_message: String,
}

/// FNV-1a 64-bit — Deterministic hash for template_id, independent of process restarts or Rust versions.
pub(crate) fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Wrapper for the Drain parsing tree — strips dynamic variables to extract static template IDs.
pub struct TemplateParser {
    tree: DrainTree,
}

impl Default for TemplateParser {
    fn default() -> Self {
        Self::new()
    }
}

impl TemplateParser {
    pub fn new() -> Self {
        Self {
            tree: DrainTree::new(),
        }
    }

    pub fn parse(&mut self, line: &str) -> ParsedLog {
        let template = self
            .tree
            .add_log_line(line)
            .map(|cluster| cluster.as_string())
            .unwrap_or_else(|| line.to_string());
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        ParsedLog {
            template_id: fnv1a64(&template),
            template,
            timestamp,
            metadata: HashMap::new(),
            raw_message: line.to_string(),
        }
    }
}

/// CPU (ONNX) based sentence embedding — all-MiniLM-L6-v2 (384-dimensional).
/// Performs mean pooling and L2 normalization.
pub struct Embedder {
    session: Session,
    tokenizer: Tokenizer,
}

impl Embedder {
    pub fn new(model_path: impl AsRef<Path>, tokenizer_path: impl AsRef<Path>) -> Result<Self> {
        let session = Session::builder()?
            .commit_from_file(model_path.as_ref())
            .context("Failed to load ONNX model")?;
        let tokenizer = Self::build_tokenizer(
            Tokenizer::from_file(tokenizer_path.as_ref())
                .map_err(|e| anyhow!("Failed to load tokenizer: {e}"))?,
        )?;
        Ok(Self { session, tokenizer })
    }

    /// Creates an Embedder from raw bytes — enables single-binary distribution via `include_bytes!`.
    pub fn from_bytes(model_bytes: &[u8], tokenizer_bytes: &[u8]) -> Result<Self> {
        let session = Session::builder()?
            .commit_from_memory(model_bytes)
            .context("Failed to load ONNX model from bytes")?;
        let tokenizer = Self::build_tokenizer(
            Tokenizer::from_bytes(tokenizer_bytes)
                .map_err(|e| anyhow!("Failed to load tokenizer from bytes: {e}"))?,
        )?;
        Ok(Self { session, tokenizer })
    }

    fn build_tokenizer(mut t: Tokenizer) -> Result<Tokenizer> {
        t.with_truncation(Some(tokenizers::TruncationParams {
            max_length: 512,
            strategy: tokenizers::TruncationStrategy::LongestFirst,
            stride: 0,
            direction: tokenizers::TruncationDirection::Right,
        }))
        .map_err(|e| anyhow!("Failed to configure tokenizer truncation: {e}"))?;
        Ok(t)
    }

    pub fn embed(&mut self, text: &str) -> Result<Vec<f32>> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow!("Failed to tokenize text: {e}"))?;
        let ids: Vec<i64> = encoding.get_ids().iter().map(|&x| i64::from(x)).collect();
        let mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&x| i64::from(x))
            .collect();
        let type_ids: Vec<i64> = encoding
            .get_type_ids()
            .iter()
            .map(|&x| i64::from(x))
            .collect();
        let len = ids.len();

        let outputs = self.session.run(ort::inputs![
            "input_ids" => Tensor::from_array(([1usize, len], ids))?,
            "attention_mask" => Tensor::from_array(([1usize, len], mask))?,
            "token_type_ids" => Tensor::from_array(([1usize, len], type_ids))?,
        ])?;
        let (shape, data) = outputs["last_hidden_state"].try_extract_tensor::<f32>()?;
        let hidden = shape[2] as usize;

        // Mean pooling based on attention mask. Pool straight from the tokenizer's u32
        // mask so the i64 copy can be moved into the tensor above (no clone).
        let attn = encoding.get_attention_mask();
        let mut vector = vec![0f32; hidden];
        let mut count = 0f32;
        for (token, &m) in attn.iter().enumerate() {
            if m == 1 {
                count += 1.0;
                let row = &data[token * hidden..(token + 1) * hidden];
                for (v, &d) in vector.iter_mut().zip(row) {
                    *v += d;
                }
            }
        }
        for v in vector.iter_mut() {
            *v /= count.max(1.0);
        }

        // L2 normalization
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in vector.iter_mut() {
                *v /= norm;
            }
        }
        Ok(vector)
    }
}

/// Drain parser + template-vector LRU cache, WITHOUT an embedder.
///
/// Separating the cheap path (parse + O(1) lookup, microseconds) from the expensive path
/// (ONNX inference, milliseconds) lets callers hold this under a short-lived lock while
/// running embeddings outside of it — a cache-miss storm then no longer blocks the
/// cache-hit ingest path (see `engine::ingest_log`).
pub struct TemplateCache {
    parser: TemplateParser,
    cache: LruCache<u64, Arc<[f32]>>,
    hits: u64,
    misses: u64,
}

impl TemplateCache {
    /// Minimum specification threshold — scales with `with_capacity` based on memory availability.
    pub const DEFAULT_CAPACITY: usize = 10_000;

    pub fn new() -> Self {
        Self::with_capacity(Self::DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            parser: TemplateParser::new(),
            cache: LruCache::new(NonZeroUsize::new(capacity.max(1)).unwrap()),
            hits: 0,
            misses: 0,
        }
    }

    /// Parses the line and looks up the template vector. `None` means a cache miss —
    /// the caller should embed `parsed.template` and store it via [`Self::insert`].
    pub fn parse_and_lookup(&mut self, log: &str) -> (ParsedLog, Option<Arc<[f32]>>) {
        let parsed = self.parser.parse(log);
        match self.cache.get(&parsed.template_id) {
            Some(vector) => {
                self.hits += 1;
                let vector = Arc::clone(vector);
                (parsed, Some(vector))
            }
            None => {
                self.misses += 1;
                (parsed, None)
            }
        }
    }

    pub fn insert(&mut self, template_id: u64, vector: Arc<[f32]>) {
        self.cache.put(template_id, vector);
    }

    /// Looks up a vector by template string WITHOUT touching the Drain tree — for
    /// re-scoring a template whose parse result (and thus template_id) is already known,
    /// avoiding a second `add_log_line` re-feed of the same line into Drain's stateful tree.
    pub fn lookup_by_template(&mut self, template: &str) -> Option<Arc<[f32]>> {
        let id = fnv1a64(template);
        match self.cache.get(&id) {
            Some(v) => {
                self.hits += 1;
                Some(Arc::clone(v))
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    pub fn hits(&self) -> u64 {
        self.hits
    }

    pub fn misses(&self) -> u64 {
        self.misses
    }

    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

impl Default for TemplateCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Template ID to vector LRU cache. Avoids embedding overhead via O(1) lookups, falling back to ONNX inference on cache misses.
///
/// Single-threaded convenience wrapper bundling [`TemplateCache`] with one [`Embedder`].
/// Concurrent pipelines (`TurboLogEngine`) use the two parts separately instead.
pub struct VectorCache {
    templates: TemplateCache,
    embedder: Embedder,
}

impl VectorCache {
    pub const DEFAULT_CAPACITY: usize = TemplateCache::DEFAULT_CAPACITY;

    pub fn new(embedder: Embedder) -> Self {
        Self::with_capacity(embedder, Self::DEFAULT_CAPACITY)
    }

    pub fn with_capacity(embedder: Embedder, capacity: usize) -> Self {
        Self {
            templates: TemplateCache::with_capacity(capacity),
            embedder,
        }
    }

    pub fn get_or_embed(&mut self, log: &str) -> Result<(ParsedLog, Arc<[f32]>)> {
        let (parsed, cached) = self.templates.parse_and_lookup(log);
        if let Some(vector) = cached {
            return Ok((parsed, vector));
        }
        let vector: Arc<[f32]> = self.embedder.embed(&parsed.template)?.into();
        self.templates
            .insert(parsed.template_id, Arc::clone(&vector));
        Ok((parsed, vector))
    }

    /// Embeds a search query — queries bypass the LRU cache since they are not templates.
    pub fn embed_uncached(&mut self, text: &str) -> Result<Vec<f32>> {
        self.embedder.embed(text)
    }

    /// Gets the vector for an already-known template, WITHOUT re-running Drain (the
    /// template is taken as given, not re-derived from a raw log line). Embeds on cache
    /// miss only — used by [`crate::pipeline::LocalPipeline::rescore`] so re-scoring a
    /// line after calibration doesn't re-feed the line into the Drain tree a second time.
    pub fn vector_for_template(&mut self, template: &str) -> Result<Arc<[f32]>> {
        if let Some(vector) = self.templates.lookup_by_template(template) {
            return Ok(vector);
        }
        let vector: Arc<[f32]> = self.embedder.embed(template)?.into();
        self.templates
            .insert(fnv1a64(template), Arc::clone(&vector));
        Ok(vector)
    }

    pub fn hits(&self) -> u64 {
        self.templates.hits()
    }

    pub fn misses(&self) -> u64 {
        self.templates.misses()
    }

    pub fn hit_rate(&self) -> f64 {
        self.templates.hit_rate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a64_deterministic() {
        assert_eq!(fnv1a64("Node <*> is online"), fnv1a64("Node <*> is online"));
        assert_ne!(fnv1a64("Node <*> is online"), fnv1a64("Node <*> offline"));
    }

    #[test]
    fn same_pattern_same_template_id() {
        let mut parser = TemplateParser::new();
        let a = parser.parse("Node 2 is online");
        let b = parser.parse("Node 4 is online");
        assert_eq!(
            a.template_id, b.template_id,
            "logs differing only in variables should share the same template"
        );
        assert_eq!(a.raw_message, "Node 2 is online");
    }

    #[test]
    fn different_pattern_different_template_id() {
        let mut parser = TemplateParser::new();
        let a = parser.parse("connection accepted from 10.0.0.1 port 5432");
        let b = parser.parse("disk usage at 91 percent on /var");
        assert_ne!(a.template_id, b.template_id);
    }
}
