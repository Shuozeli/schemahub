//! Small, dependency-light production observability primitives.
//!
//! Request metrics are process-local and rendered in the Prometheus text
//! exposition format. Durable audit history remains in JJ/ChangeRecord data;
//! these counters are operational signals and intentionally reset on restart.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

const HTTP_DURATION_BUCKETS_SECONDS: [f64; 11] = [
    0.005, 0.010, 0.025, 0.050, 0.100, 0.250, 0.500, 1.0, 2.5, 5.0, 10.0,
];

#[derive(Clone, Debug)]
pub struct ServerMetrics {
    inner: Arc<MetricsInner>,
}

#[derive(Debug)]
struct MetricsInner {
    http_requests_total: AtomicU64,
    http_requests_in_flight: AtomicU64,
    http_requests_cancelled_total: AtomicU64,
    http_responses_by_class: [AtomicU64; 5],
    http_duration_buckets: [AtomicU64; HTTP_DURATION_BUCKETS_SECONDS.len()],
    http_duration_microseconds_total: AtomicU64,
    grpc_requests_total: AtomicU64,
    readiness_ready_total: AtomicU64,
    readiness_draining_total: AtomicU64,
    readiness_auth_failure_total: AtomicU64,
    readiness_storage_failure_total: AtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadinessResult {
    Ready,
    Draining,
    AuthFailure,
    StorageFailure,
}

impl Default for ServerMetrics {
    fn default() -> Self {
        Self {
            inner: Arc::new(MetricsInner {
                http_requests_total: AtomicU64::new(0),
                http_requests_in_flight: AtomicU64::new(0),
                http_requests_cancelled_total: AtomicU64::new(0),
                http_responses_by_class: std::array::from_fn(|_| AtomicU64::new(0)),
                http_duration_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
                http_duration_microseconds_total: AtomicU64::new(0),
                grpc_requests_total: AtomicU64::new(0),
                readiness_ready_total: AtomicU64::new(0),
                readiness_draining_total: AtomicU64::new(0),
                readiness_auth_failure_total: AtomicU64::new(0),
                readiness_storage_failure_total: AtomicU64::new(0),
            }),
        }
    }
}

impl ServerMetrics {
    pub fn record_grpc_request(&self) {
        self.inner
            .grpc_requests_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_readiness(&self, result: ReadinessResult) {
        let counter = match result {
            ReadinessResult::Ready => &self.inner.readiness_ready_total,
            ReadinessResult::Draining => &self.inner.readiness_draining_total,
            ReadinessResult::AuthFailure => &self.inner.readiness_auth_failure_total,
            ReadinessResult::StorageFailure => &self.inner.readiness_storage_failure_total,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn render_prometheus(&self) -> String {
        let mut output = String::with_capacity(2_048);
        writeln!(
            output,
            "# HELP schemahub_build_info SchemaHub server build information.\n\
             # TYPE schemahub_build_info gauge\n\
             schemahub_build_info{{version=\"{}\"}} 1",
            crate::BUILD_VERSION
        )
        .expect("writing metrics to String");
        append_atomic_metric(
            &mut output,
            "schemahub_http_requests_total",
            "Total HTTP requests accepted by the BFF.",
            "counter",
            &self.inner.http_requests_total,
        );
        append_atomic_metric(
            &mut output,
            "schemahub_http_requests_in_flight",
            "HTTP requests currently executing in the BFF.",
            "gauge",
            &self.inner.http_requests_in_flight,
        );
        append_atomic_metric(
            &mut output,
            "schemahub_http_requests_cancelled_total",
            "HTTP requests cancelled before a response was produced.",
            "counter",
            &self.inner.http_requests_cancelled_total,
        );
        append_atomic_metric(
            &mut output,
            "schemahub_grpc_requests_total",
            "Total gRPC requests accepted by the server.",
            "counter",
            &self.inner.grpc_requests_total,
        );

        output.push_str(
            "# HELP schemahub_http_responses_total HTTP responses by status-code class.\n\
             # TYPE schemahub_http_responses_total counter\n",
        );
        for (index, class) in ["1xx", "2xx", "3xx", "4xx", "5xx"].iter().enumerate() {
            writeln!(
                output,
                "schemahub_http_responses_total{{class=\"{class}\"}} {}",
                self.inner.http_responses_by_class[index].load(Ordering::Relaxed)
            )
            .expect("writing metrics to String");
        }

        output.push_str(
            "# HELP schemahub_http_request_duration_seconds HTTP response latency.\n\
             # TYPE schemahub_http_request_duration_seconds histogram\n",
        );
        for (index, upper_bound) in HTTP_DURATION_BUCKETS_SECONDS.iter().enumerate() {
            writeln!(
                output,
                "schemahub_http_request_duration_seconds_bucket{{le=\"{upper_bound}\"}} {}",
                self.inner.http_duration_buckets[index].load(Ordering::Relaxed)
            )
            .expect("writing metrics to String");
        }
        let request_count = self.inner.http_requests_total.load(Ordering::Relaxed)
            - self
                .inner
                .http_requests_cancelled_total
                .load(Ordering::Relaxed);
        writeln!(
            output,
            "schemahub_http_request_duration_seconds_bucket{{le=\"+Inf\"}} {request_count}"
        )
        .expect("writing metrics to String");
        writeln!(
            output,
            "schemahub_http_request_duration_seconds_sum {:.6}",
            self.inner
                .http_duration_microseconds_total
                .load(Ordering::Relaxed) as f64
                / 1_000_000.0
        )
        .expect("writing metrics to String");
        writeln!(
            output,
            "schemahub_http_request_duration_seconds_count {request_count}"
        )
        .expect("writing metrics to String");

        output.push_str(
            "# HELP schemahub_readiness_checks_total Readiness probes by outcome.\n\
             # TYPE schemahub_readiness_checks_total counter\n",
        );
        for (result, counter) in [
            ("ready", &self.inner.readiness_ready_total),
            ("draining", &self.inner.readiness_draining_total),
            ("auth_failure", &self.inner.readiness_auth_failure_total),
            (
                "storage_failure",
                &self.inner.readiness_storage_failure_total,
            ),
        ] {
            writeln!(
                output,
                "schemahub_readiness_checks_total{{result=\"{result}\"}} {}",
                counter.load(Ordering::Relaxed)
            )
            .expect("writing metrics to String");
        }
        output
    }

    fn start_http_request(&self) -> HttpRequestGuard {
        self.inner
            .http_requests_total
            .fetch_add(1, Ordering::Relaxed);
        self.inner
            .http_requests_in_flight
            .fetch_add(1, Ordering::Relaxed);
        HttpRequestGuard {
            metrics: self.clone(),
            started_at: Instant::now(),
            completed: false,
        }
    }

    fn complete_http_request(&self, status: u16, elapsed_seconds: f64) {
        if let Some(index) = status
            .checked_div(100)
            .and_then(|class| class.checked_sub(1))
            .filter(|class| *class < 5)
        {
            self.inner.http_responses_by_class[index as usize].fetch_add(1, Ordering::Relaxed);
        }
        for (index, upper_bound) in HTTP_DURATION_BUCKETS_SECONDS.iter().enumerate() {
            if elapsed_seconds <= *upper_bound {
                self.inner.http_duration_buckets[index].fetch_add(1, Ordering::Relaxed);
            }
        }
        let elapsed_microseconds = (elapsed_seconds * 1_000_000.0).round() as u64;
        self.inner
            .http_duration_microseconds_total
            .fetch_add(elapsed_microseconds, Ordering::Relaxed);
    }
}

struct HttpRequestGuard {
    metrics: ServerMetrics,
    started_at: Instant,
    completed: bool,
}

impl HttpRequestGuard {
    fn complete(mut self, status: u16) {
        self.metrics
            .complete_http_request(status, self.started_at.elapsed().as_secs_f64());
        self.completed = true;
    }
}

impl Drop for HttpRequestGuard {
    fn drop(&mut self) {
        self.metrics
            .inner
            .http_requests_in_flight
            .fetch_sub(1, Ordering::Relaxed);
        if !self.completed {
            self.metrics
                .inner
                .http_requests_cancelled_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub async fn track_http_requests(
    State(metrics): State<ServerMetrics>,
    request: Request,
    next: Next,
) -> Response {
    let request_guard = metrics.start_http_request();
    let response = next.run(request).await;
    request_guard.complete(response.status().as_u16());
    response
}

fn append_atomic_metric(
    output: &mut String,
    name: &str,
    help: &str,
    metric_type: &str,
    value: &AtomicU64,
) {
    writeln!(
        output,
        "# HELP {name} {help}\n# TYPE {name} {metric_type}\n{name} {}",
        value.load(Ordering::Relaxed)
    )
    .expect("writing metrics to String");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prometheus_render_includes_cumulative_latency_and_readiness_outcomes() {
        // Arrange
        let metrics = ServerMetrics::default();
        metrics.record_readiness(ReadinessResult::Ready);
        metrics.record_readiness(ReadinessResult::AuthFailure);
        metrics.record_grpc_request();
        metrics.complete_http_request(200, 0.075);
        metrics
            .inner
            .http_requests_total
            .fetch_add(1, Ordering::Relaxed);

        // Act
        let rendered = metrics.render_prometheus();

        // Assert
        assert!(rendered.contains("schemahub_http_responses_total{class=\"2xx\"} 1"));
        assert!(rendered.contains("schemahub_grpc_requests_total 1"));
        assert!(rendered.contains("schemahub_http_request_duration_seconds_bucket{le=\"0.05\"} 0"));
        assert!(rendered.contains("schemahub_http_request_duration_seconds_bucket{le=\"0.1\"} 1"));
        assert!(rendered.contains("schemahub_readiness_checks_total{result=\"ready\"} 1"));
        assert!(rendered.contains("schemahub_readiness_checks_total{result=\"auth_failure\"} 1"));
    }
}
