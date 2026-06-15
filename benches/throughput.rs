//! Criterion 벤치마크 — ONNX 모델 비의존 핫패스 측정
//!
//! 모델(ONNX) 없이 CI에서도 돌아야 하므로, 실제 임베딩 추론이 필요한
//! Embedder / VectorCache 는 여기서 측정하지 않는다.
//! 측정 대상: TemplateParser, TemplateCache 적중 경로, AnomalyDetector, PingPongIndexer

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use turbolog::detect::AnomalyDetector;
use turbolog::index::PingPongIndexer;
use turbolog::ingest::{TemplateCache, TemplateParser};

// ── 합성 로그 라인 생성 ──────────────────────────────────────────────────────

fn make_line(i: usize) -> String {
    match i % 10 {
        0 => format!(
            "connection accepted from 10.0.{}.{} port {}",
            i % 256,
            (i * 7) % 256,
            5000 + i % 1000
        ),
        1 => format!("user u{} login success from web console", i),
        2 => format!("disk usage at {} percent on /var", i % 100),
        3 => format!("request to /api/v1/items took {} ms", i % 900),
        4 => format!("worker {} heartbeat ok at epoch {}", i % 64, 1700000000 + i),
        5 => format!("cache evicted {} entries in shard {}", i % 5000, i % 16),
        6 => format!("gc pause of {} ms in region old-gen", i % 300),
        7 => format!("tcp retransmit count {} on eth0", i % 99),
        8 => format!("query plan hash {} executed in {} ms", i * 31, i % 50),
        _ => format!(
            "session {} renewed token expiring in {} s",
            i,
            3600 - i % 600
        ),
    }
}

// ── 더미 384차원 단위벡터 ────────────────────────────────────────────────────

fn unit_vec(seed: usize, dim: usize) -> Vec<f32> {
    let mut v: Vec<f32> = (0..dim)
        .map(|j| {
            // LCG 기반 결정론적 난수
            let x = (seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(j.wrapping_mul(2891336453))
                >> 16) as f32;
            (x / u32::MAX as f32) * 2.0 - 1.0
        })
        .collect();
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

/// TemplateCache를 N개 템플릿이 삽입된 상태로 새로 만드는 팩토리.
/// TemplateCache가 Clone을 impl하지 않으므로 iter_batched의 setup fn으로 사용.
fn make_warmed_cache(n: usize, dim: usize) -> (TemplateCache, Vec<String>) {
    let mut cache = TemplateCache::new();
    let mut parser = TemplateParser::new();
    for i in 0..n {
        let parsed = parser.parse(&make_line(i));
        let vec: Arc<[f32]> = unit_vec(i, dim).into();
        cache.insert(parsed.template_id, vec);
    }
    // 조회용 라인 (모두 캐시에 있는 템플릿)
    let lookup_lines: Vec<String> = (0..1000).map(|i| make_line(i % n)).collect();
    (cache, lookup_lines)
}

// ── 벤치: TemplateParser::parse ──────────────────────────────────────────────

fn bench_template_parser(c: &mut Criterion) {
    let lines: Vec<String> = (0..100).map(make_line).collect();

    let mut group = c.benchmark_group("template_parser");
    group.throughput(Throughput::Elements(lines.len() as u64));
    group.bench_function("parse_100", |b| {
        b.iter_batched(
            TemplateParser::new,
            |mut parser| {
                for line in &lines {
                    let _ = parser.parse(line);
                }
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

// ── 벤치: TemplateCache 적중 경로 ────────────────────────────────────────────

fn bench_template_cache_hit(c: &mut Criterion) {
    const DIM: usize = 384;
    const N: usize = 10; // 10개 고정 템플릿

    let mut group = c.benchmark_group("template_cache");
    group.throughput(Throughput::Elements(1000));

    group.bench_function("hit_1000", |b| {
        // 매 반복마다 setup에서 warmed cache를 새로 구성 (Clone 불필요)
        b.iter_batched(
            || make_warmed_cache(N, DIM),
            |(mut cache, lookup_lines)| {
                for line in &lookup_lines {
                    let _ = cache.parse_and_lookup(line);
                }
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

// ── 벤치: AnomalyDetector::fit + detect + min_distance ──────────────────────

fn bench_anomaly_detector(c: &mut Criterion) {
    const DIM: usize = 384;
    const N: usize = 200; // fit용 샘플 수
    const K: usize = 10; // 센트로이드 수

    // 합성 n×dim flat 벡터
    let flat: Vec<f32> = (0..N).flat_map(|i| unit_vec(i, DIM)).collect();
    let probe = unit_vec(999, DIM);

    let mut group = c.benchmark_group("anomaly_detector");

    // fit 벤치
    group.throughput(Throughput::Elements(N as u64));
    group.bench_function(
        BenchmarkId::new("fit", format!("n{N}_k{K}_dim{DIM}")),
        |b| {
            b.iter(|| {
                let _ = AnomalyDetector::fit(&flat, DIM, K, 0.5);
            })
        },
    );

    // min_distance 벤치 (미리 fit한 detector 재사용)
    let detector = AnomalyDetector::fit(&flat, DIM, K, 0.5);
    group.throughput(Throughput::Elements(1000));
    group.bench_function("min_distance_1000", |b| {
        b.iter(|| {
            for _ in 0..1000 {
                let _ = detector.min_distance(&probe);
            }
        })
    });

    group.finish();
}

// ── 벤치: PingPongIndexer::ingest + search ───────────────────────────────────

fn bench_pingpong_indexer(c: &mut Criterion) {
    const DIM: usize = 384;
    const BIT_WIDTH: usize = 4;
    const INGEST_N: usize = 1000;

    let vectors: Vec<Vec<f32>> = (0..INGEST_N).map(|i| unit_vec(i, DIM)).collect();
    let query = unit_vec(9999, DIM);

    let mut group = c.benchmark_group("pingpong_indexer");

    // ingest 벤치
    group.throughput(Throughput::Elements(INGEST_N as u64));
    group.bench_function(
        BenchmarkId::new("ingest", format!("n{INGEST_N}_dim{DIM}")),
        |b| {
            b.iter_batched(
                || PingPongIndexer::new(DIM, BIT_WIDTH).unwrap(),
                |idx| {
                    for (i, v) in vectors.iter().enumerate() {
                        idx.ingest(i as u64, v).unwrap();
                    }
                },
                BatchSize::SmallInput,
            )
        },
    );

    // search 벤치: 미리 1000개 ingest + swap_and_flush 후 search 100회
    let idx = PingPongIndexer::new(DIM, BIT_WIDTH).unwrap();
    for (i, v) in vectors.iter().enumerate() {
        idx.ingest(i as u64, v).unwrap();
    }
    idx.swap_and_flush(None).unwrap();

    group.throughput(Throughput::Elements(100));
    group.bench_function(BenchmarkId::new("search", format!("n{INGEST_N}_k5")), |b| {
        b.iter(|| {
            for _ in 0..100 {
                let search_idx = idx.get_search_index();
                let _ = search_idx.search(&query, 5);
            }
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_template_parser,
    bench_template_cache_hit,
    bench_anomaly_detector,
    bench_pingpong_indexer,
);
criterion_main!(benches);
