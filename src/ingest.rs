//! Ingestion & Cache Layer — 텍스트 로그를 벡터로 변환하는 관문.
//!
//! Cache Hit 시 임베딩 연산 비용 0, Cache Miss 시에만 CPU(ONNX) 추론을 수행한다.
//!
//! 시스템 제약 (스펙 v1.0 §4.3 — Stateless Embedder):
//! `Embedder`는 요청 간 상태를 갖지 않는다. 부하 증가 시 코어 엔진과 분리된
//! 스레드 풀에서 인스턴스를 횡적으로 확장할 수 있어야 한다.

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

/// Drain 파싱을 거친 구조화 로그.
pub struct ParsedLog {
    pub template_id: u64,
    /// 동적 변수가 `<*>`로 치환된 정적 템플릿 문자열 (임베딩 입력).
    pub template: String,
    /// 인입 시각 (Unix epoch 초).
    pub timestamp: i64,
    pub metadata: HashMap<String, String>,
    pub raw_message: String,
}

/// FNV-1a 64-bit — 프로세스 재시작·Rust 버전과 무관하게 결정적인 template_id 해시.
fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Drain 알고리즘 래퍼 — 로그의 동적 변수를 지우고 정적 템플릿 ID를 추출한다.
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

/// CPU(ONNX) 기반 문장 임베딩 — all-MiniLM-L6-v2 (384차원).
/// mean pooling + L2 정규화 수행.
pub struct Embedder {
    session: Session,
    tokenizer: Tokenizer,
}

impl Embedder {
    pub fn new(model_path: impl AsRef<Path>, tokenizer_path: impl AsRef<Path>) -> Result<Self> {
        let session = Session::builder()?
            .commit_from_file(model_path.as_ref())
            .context("ONNX 모델 로드 실패")?;
        let tokenizer = Tokenizer::from_file(tokenizer_path.as_ref())
            .map_err(|e| anyhow!("토크나이저 로드 실패: {e}"))?;
        Ok(Self { session, tokenizer })
    }

    pub fn embed(&mut self, text: &str) -> Result<Vec<f32>> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow!("토크나이즈 실패: {e}"))?;
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
            "attention_mask" => Tensor::from_array(([1usize, len], mask.clone()))?,
            "token_type_ids" => Tensor::from_array(([1usize, len], type_ids))?,
        ])?;
        let (shape, data) = outputs["last_hidden_state"].try_extract_tensor::<f32>()?;
        let hidden = shape[2] as usize;

        // attention mask 기반 mean pooling
        let mut vector = vec![0f32; hidden];
        let mut count = 0f32;
        for (token, &m) in mask.iter().enumerate() {
            if m == 1 {
                count += 1.0;
                let offset = token * hidden;
                for (j, v) in vector.iter_mut().enumerate() {
                    *v += data[offset + j];
                }
            }
        }
        for v in vector.iter_mut() {
            *v /= count.max(1.0);
        }

        // L2 정규화
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in vector.iter_mut() {
                *v /= norm;
            }
        }
        Ok(vector)
    }
}

/// 템플릿 ID → 벡터 LRU 캐시. O(1) 룩업으로 임베딩 비용을 회피하거나 ONNX 추론을 실행한다.
pub struct VectorCache {
    parser: TemplateParser,
    cache: LruCache<u64, Arc<[f32]>>,
    embedder: Embedder,
    hits: u64,
    misses: u64,
}

impl VectorCache {
    /// 스펙 최소치 — 메모리 가용량에 따라 `with_capacity`로 상향.
    pub const DEFAULT_CAPACITY: usize = 10_000;

    pub fn new(embedder: Embedder) -> Self {
        Self::with_capacity(embedder, Self::DEFAULT_CAPACITY)
    }

    pub fn with_capacity(embedder: Embedder, capacity: usize) -> Self {
        Self {
            parser: TemplateParser::new(),
            cache: LruCache::new(NonZeroUsize::new(capacity.max(1)).unwrap()),
            embedder,
            hits: 0,
            misses: 0,
        }
    }

    pub fn get_or_embed(&mut self, log: &str) -> Result<(ParsedLog, Arc<[f32]>)> {
        let parsed = self.parser.parse(log);
        if let Some(vector) = self.cache.get(&parsed.template_id) {
            self.hits += 1;
            return Ok((parsed, Arc::clone(vector)));
        }
        self.misses += 1;
        let vector: Arc<[f32]> = self.embedder.embed(&parsed.template)?.into();
        self.cache.put(parsed.template_id, Arc::clone(&vector));
        Ok((parsed, vector))
    }

    /// 검색 쿼리용 임베딩 — 쿼리는 템플릿이 아니므로 캐시를 거치지 않는다.
    pub fn embed_uncached(&mut self, text: &str) -> Result<Vec<f32>> {
        self.embedder.embed(text)
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
        assert_eq!(a.template_id, b.template_id, "변수만 다른 로그는 같은 템플릿");
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
