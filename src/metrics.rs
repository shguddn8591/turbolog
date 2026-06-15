//! Process-wide metrics registry — Prometheus text exposition, dependency-free.
//!
//! Every counter, gauge and histogram is a process-global atomic. The engine and HTTP
//! layers call the free functions here on the hot path (allocation- and lock-free); the
//! `/metrics` endpoint renders the registry via [`render`]. Kept free of external crates
//! so instrumentation never costs more than a relaxed atomic add.

use std::fmt::Write;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

// ── Counters & gauges ────────────────────────────────────────────────────────
static INGESTED: AtomicU64 = AtomicU64::new(0);
static ANOMALIES: AtomicU64 = AtomicU64::new(0);
static HTTP_2XX: AtomicU64 = AtomicU64::new(0);
static HTTP_4XX: AtomicU64 = AtomicU64::new(0);
static HTTP_5XX: AtomicU64 = AtomicU64::new(0);
static HTTP_REJECTED: AtomicU64 = AtomicU64::new(0);
static INFLIGHT: AtomicI64 = AtomicI64::new(0);

/// Logs accepted by the ingest path.
pub fn inc_ingested(n: u64) {
    INGESTED.fetch_add(n, Ordering::Relaxed);
}

/// An ingested log classified as an anomaly.
pub fn inc_anomaly() {
    ANOMALIES.fetch_add(1, Ordering::Relaxed);
}

/// A served HTTP response, bucketed by status class.
pub fn inc_http(status: u16) {
    let counter = match status {
        200..=299 => &HTTP_2XX,
        400..=499 => &HTTP_4XX,
        500..=599 => &HTTP_5XX,
        _ => return,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

/// A request shed by backpressure before handling (HTTP 503).
pub fn inc_http_rejected() {
    HTTP_REJECTED.fetch_add(1, Ordering::Relaxed);
}

/// In-flight request gauge — paired `inc`/`dec` around request handling.
pub fn inflight_inc() {
    INFLIGHT.fetch_add(1, Ordering::Relaxed);
}

/// See [`inflight_inc`].
pub fn inflight_dec() {
    INFLIGHT.fetch_sub(1, Ordering::Relaxed);
}

// ── Latency histograms ───────────────────────────────────────────────────────
/// Bucket upper bounds in seconds (Prometheus `le`), spanning 50µs … 5s.
const BUCKETS: [f64; 16] = [
    0.00005, 0.0001, 0.00025, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0,
    2.5, 5.0,
];

/// Fixed-bucket latency histogram. Each `observe` increments exactly one bucket
/// (non-cumulative storage); [`Histogram::render`] emits cumulative `le` counts.
struct Histogram {
    counts: [AtomicU64; 16],
    sum_nanos: AtomicU64,
    count: AtomicU64,
}

impl Histogram {
    const fn new() -> Self {
        Self {
            counts: [const { AtomicU64::new(0) }; 16],
            sum_nanos: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    fn observe(&self, seconds: f64) {
        for (i, &bound) in BUCKETS.iter().enumerate() {
            if seconds <= bound {
                self.counts[i].fetch_add(1, Ordering::Relaxed);
                break;
            }
        }
        self.sum_nanos
            .fetch_add((seconds * 1e9) as u64, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    fn render(&self, name: &str, help: &str, out: &mut String) {
        let _ = writeln!(out, "# HELP {name} {help}");
        let _ = writeln!(out, "# TYPE {name} histogram");
        let mut cumulative = 0u64;
        for (i, &bound) in BUCKETS.iter().enumerate() {
            cumulative += self.counts[i].load(Ordering::Relaxed);
            let _ = writeln!(out, "{name}_bucket{{le=\"{bound}\"}} {cumulative}");
        }
        let total = self.count.load(Ordering::Relaxed);
        let _ = writeln!(out, "{name}_bucket{{le=\"+Inf\"}} {total}");
        let sum = self.sum_nanos.load(Ordering::Relaxed) as f64 / 1e9;
        let _ = writeln!(out, "{name}_sum {sum}");
        let _ = writeln!(out, "{name}_count {total}");
    }
}

static INGEST_HIST: Histogram = Histogram::new();
static SEARCH_HIST: Histogram = Histogram::new();

/// Observe an ingest-path latency sample (seconds).
pub fn observe_ingest_seconds(seconds: f64) {
    INGEST_HIST.observe(seconds);
}

/// Observe a search-path latency sample (seconds).
pub fn observe_search_seconds(seconds: f64) {
    SEARCH_HIST.observe(seconds);
}

// ── Exposition ───────────────────────────────────────────────────────────────
fn counter(out: &mut String, name: &str, help: &str, value: u64) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} counter");
    let _ = writeln!(out, "{name} {value}");
}

fn gauge(out: &mut String, name: &str, help: &str, value: f64) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} gauge");
    let _ = writeln!(out, "{name} {value}");
}

/// Renders the full registry in Prometheus text format. `extra_gauges` lets callers
/// append point-in-time gauges sourced elsewhere — e.g. the HTTP layer passes engine
/// stats (cache hit rate, ring depth) as `(name, help, value)` tuples.
pub fn render(extra_gauges: &[(&str, &str, f64)]) -> String {
    let mut out = String::with_capacity(4096);
    counter(
        &mut out,
        "turbolog_ingested_total",
        "Logs ingested",
        INGESTED.load(Ordering::Relaxed),
    );
    counter(
        &mut out,
        "turbolog_anomalies_total",
        "Anomalies detected",
        ANOMALIES.load(Ordering::Relaxed),
    );
    counter(
        &mut out,
        "turbolog_http_requests_2xx_total",
        "HTTP 2xx responses",
        HTTP_2XX.load(Ordering::Relaxed),
    );
    counter(
        &mut out,
        "turbolog_http_requests_4xx_total",
        "HTTP 4xx responses",
        HTTP_4XX.load(Ordering::Relaxed),
    );
    counter(
        &mut out,
        "turbolog_http_requests_5xx_total",
        "HTTP 5xx responses",
        HTTP_5XX.load(Ordering::Relaxed),
    );
    counter(
        &mut out,
        "turbolog_http_rejected_total",
        "Requests shed by backpressure",
        HTTP_REJECTED.load(Ordering::Relaxed),
    );
    gauge(
        &mut out,
        "turbolog_inflight_requests",
        "In-flight HTTP requests",
        INFLIGHT.load(Ordering::Relaxed) as f64,
    );
    INGEST_HIST.render(
        "turbolog_ingest_latency_seconds",
        "Ingest path latency",
        &mut out,
    );
    SEARCH_HIST.render(
        "turbolog_search_latency_seconds",
        "Search path latency",
        &mut out,
    );
    for (name, help, value) in extra_gauges {
        gauge(&mut out, name, help, *value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_buckets_are_cumulative() {
        let h = Histogram::new();
        h.observe(0.0003); // falls in le=0.0005
        h.observe(0.002); // falls in le=0.0025
        h.observe(10.0); // beyond last finite bucket -> only +Inf
        let mut out = String::new();
        h.render("t", "test", &mut out);
        assert!(out.contains("t_count 3"));
        assert!(out.contains("t_bucket{le=\"+Inf\"} 3"));
        // cumulative: le=0.0005 has 1, le=0.0025 has 2
        assert!(out.contains("t_bucket{le=\"0.0005\"} 1"));
        assert!(out.contains("t_bucket{le=\"0.0025\"} 2"));
    }

    #[test]
    fn render_includes_extra_gauges() {
        let text = render(&[("turbolog_cache_hit_rate", "Cache hit rate", 0.95)]);
        assert!(text.contains("turbolog_cache_hit_rate 0.95"));
        assert!(text.contains("turbolog_ingested_total"));
    }
}
