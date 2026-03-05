use axum::{extract::Request, middleware::Next, response::Response};
use metrics::{counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::path::Path;
use std::time::Instant;
use tokio::fs;

/// Install the Prometheus metrics recorder and register all metric descriptions.
/// Returns the handle used to render the /metrics endpoint.
pub fn install_recorder() -> metrics_exporter_prometheus::PrometheusHandle {
    let handle = PrometheusBuilder::new()
        .install_recorder()
        .expect("failed to install Prometheus recorder");

    register_descriptions();

    gauge!("sw1nn_pkg_repo_build_info", "version" => env!("CARGO_PKG_VERSION")).set(1.0);

    handle
}

fn register_descriptions() {
    describe_gauge!(
        "sw1nn_pkg_repo_build_info",
        "Build information (version label), always 1"
    );

    // Gauges (updated by background collector)
    describe_gauge!("sw1nn_pkg_repo_packages_total", "Total number of packages");
    describe_gauge!("sw1nn_pkg_repo_repos_total", "Number of repositories");
    describe_gauge!(
        "sw1nn_pkg_repo_storage_bytes",
        "Disk usage in bytes by kind"
    );
    describe_gauge!(
        "sw1nn_pkg_repo_upload_sessions_active",
        "Number of in-flight upload sessions"
    );
    describe_gauge!(
        "sw1nn_pkg_repo_db_pending_updates",
        "Number of pending debounced DB rebuilds"
    );

    // Counters
    describe_counter!("sw1nn_pkg_repo_http_requests_total", "Total HTTP requests");
    describe_counter!(
        "sw1nn_pkg_repo_uploads_completed_total",
        "Total successfully completed uploads"
    );
    describe_counter!(
        "sw1nn_pkg_repo_uploads_aborted_total",
        "Total aborted uploads"
    );
    describe_counter!(
        "sw1nn_pkg_repo_package_downloads_total",
        "Total package file downloads"
    );
    describe_counter!(
        "sw1nn_pkg_repo_packages_deleted_total",
        "Total packages deleted"
    );
    describe_counter!(
        "sw1nn_pkg_repo_db_rebuilds_total",
        "Total database rebuilds"
    );
    describe_counter!(
        "sw1nn_pkg_repo_cleanup_versions_deleted_total",
        "Total package versions deleted by cleanup"
    );

    // Histograms
    describe_histogram!(
        "sw1nn_pkg_repo_http_request_duration_seconds",
        "HTTP request duration in seconds"
    );
    describe_histogram!(
        "sw1nn_pkg_repo_upload_size_bytes",
        "Size of completed uploads in bytes"
    );
    describe_histogram!(
        "sw1nn_pkg_repo_db_rebuild_duration_seconds",
        "Database rebuild duration in seconds"
    );
}

/// RAII timer that records elapsed time to a histogram on drop.
pub struct ScopedTimer {
    start: Instant,
    name: &'static str,
    labels: Vec<(&'static str, String)>,
}

impl ScopedTimer {
    pub fn new(name: &'static str, labels: Vec<(&'static str, String)>) -> Self {
        Self {
            start: Instant::now(),
            name,
            labels,
        }
    }

    pub fn http_request(method: String, endpoint: String) -> Self {
        Self::new(
            "sw1nn_pkg_repo_http_request_duration_seconds",
            vec![("method", method), ("endpoint", endpoint)],
        )
    }

    pub fn db_rebuild(repo: String, arch: String) -> Self {
        Self::new(
            "sw1nn_pkg_repo_db_rebuild_duration_seconds",
            vec![("repo", repo), ("arch", arch)],
        )
    }
}

impl Drop for ScopedTimer {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed().as_secs_f64();
        let labels: Vec<metrics::Label> = self
            .labels
            .iter()
            .map(|(k, v)| metrics::Label::new(*k, v.clone()))
            .collect();
        histogram!(self.name, labels).record(elapsed);
    }
}

// -- Counter helpers --

pub fn record_http_request(method: &str, endpoint: &str, status: u16) {
    counter!(
        "sw1nn_pkg_repo_http_requests_total",
        "method" => method.to_owned(),
        "endpoint" => endpoint.to_owned(),
        "status" => status.to_string()
    )
    .increment(1);
}

pub fn record_upload_completed(repo: &str) {
    counter!("sw1nn_pkg_repo_uploads_completed_total", "repo" => repo.to_owned()).increment(1);
}

pub fn record_upload_aborted() {
    counter!("sw1nn_pkg_repo_uploads_aborted_total").increment(1);
}

pub fn record_package_download(repo: &str, arch: &str) {
    counter!(
        "sw1nn_pkg_repo_package_downloads_total",
        "repo" => repo.to_owned(),
        "arch" => arch.to_owned()
    )
    .increment(1);
}

pub fn record_package_deleted(repo: &str, count: u64) {
    counter!("sw1nn_pkg_repo_packages_deleted_total", "repo" => repo.to_owned()).increment(count);
}

pub fn record_db_rebuild(repo: &str, arch: &str, result: &str) {
    counter!(
        "sw1nn_pkg_repo_db_rebuilds_total",
        "repo" => repo.to_owned(),
        "arch" => arch.to_owned(),
        "result" => result.to_owned()
    )
    .increment(1);
}

pub fn record_cleanup_versions_deleted(repo: &str, count: u64) {
    counter!(
        "sw1nn_pkg_repo_cleanup_versions_deleted_total",
        "repo" => repo.to_owned()
    )
    .increment(count);
}

// -- Histogram helpers --

pub fn record_upload_size(repo: &str, size: u64) {
    histogram!("sw1nn_pkg_repo_upload_size_bytes", "repo" => repo.to_owned()).record(size as f64);
}

// -- Gauge helpers --

pub fn set_upload_sessions_active(count: usize) {
    gauge!("sw1nn_pkg_repo_upload_sessions_active").set(count as f64);
}

pub fn set_db_pending_updates(count: usize) {
    gauge!("sw1nn_pkg_repo_db_pending_updates").set(count as f64);
}

/// Walk a directory tree and return the total size in bytes.
async fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(mut entries) = fs::read_dir(path).await else {
        return 0;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        if meta.is_dir() {
            total += Box::pin(dir_size(&entry.path())).await;
        } else {
            total += meta.len();
        }
    }
    total
}

/// Collect storage gauge metrics by scanning the data directory.
pub async fn collect_storage_gauges(storage: &crate::storage::Storage) {
    let Ok(repos) = storage.list_repos().await else {
        return;
    };

    gauge!("sw1nn_pkg_repo_repos_total").set(repos.len() as f64);

    for repo in &repos {
        let Ok(packages) = storage.list_packages(repo).await else {
            continue;
        };

        // Aggregate package count by arch
        let mut arch_counts: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        for pkg in &packages {
            *arch_counts.entry(pkg.arch.clone()).or_default() += 1;
        }
        for (arch, count) in &arch_counts {
            gauge!(
                "sw1nn_pkg_repo_packages_total",
                "repo" => repo.clone(),
                "arch" => arch.clone()
            )
            .set(*count as f64);
        }

        // Disk usage by kind
        if let Ok(pkg_dir) = storage.packages_dir(repo) {
            let size = dir_size(&pkg_dir).await;
            gauge!(
                "sw1nn_pkg_repo_storage_bytes",
                "repo" => repo.clone(),
                "kind" => "packages"
            )
            .set(size as f64);
        }

        if let Ok(meta_dir) = storage.metadata_dir(repo) {
            let size = dir_size(&meta_dir).await;
            gauge!(
                "sw1nn_pkg_repo_storage_bytes",
                "repo" => repo.clone(),
                "kind" => "metadata"
            )
            .set(size as f64);
        }
    }
}

/// Normalise a request path into a low-cardinality endpoint label.
///
/// Replaces dynamic path segments (UUIDs, package names, versions, arch) with
/// placeholders so that Prometheus doesn't create unbounded label series.
fn normalise_endpoint(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').collect();

    // /api/packages/upload/{upload_id}/chunks/{chunk_number}
    // /api/packages/upload/{upload_id}/signature
    // /api/packages/upload/{upload_id}/complete
    // /api/packages/upload/{upload_id}          (DELETE abort)
    if segments.len() >= 5 && segments.get(3) == Some(&"upload") {
        let mut out = "/api/packages/upload/:upload_id".to_owned();
        if let Some(&tail) = segments.get(5) {
            if tail == "chunks" {
                out.push_str("/chunks/:chunk_number");
            } else {
                out.push('/');
                out.push_str(tail);
            }
        }
        return out;
    }

    // /api/packages/{name}/versions/delete
    if segments.len() >= 5 && segments.get(4) == Some(&"versions") {
        return "/api/packages/:name/versions/delete".to_owned();
    }

    // /api/packages/{name}  (DELETE)
    if segments.len() == 4 && segments.get(2) == Some(&"packages") {
        return "/api/packages/:name".to_owned();
    }

    // /api/repos/{repo}/os/{arch}/rebuild
    if segments.len() >= 6 && segments.get(2) == Some(&"repos") {
        return "/api/repos/:repo/os/:arch/rebuild".to_owned();
    }

    // /{repo}/os/{arch}/{filename}  (pacman download)
    if segments.len() == 5 && segments.get(2) == Some(&"os") {
        return "/:repo/os/:arch/:filename".to_owned();
    }

    // Static routes: /api/packages, /api/packages/cleanup, /metrics, /api-docs, /auth/*
    path.to_owned()
}

/// Axum middleware that records HTTP request count and duration.
pub async fn http_metrics_layer(request: Request, next: Next) -> Response {
    let method = request.method().to_string();
    let path = request.uri().path().to_owned();

    // Skip the /metrics endpoint itself
    if path == "/metrics" {
        return next.run(request).await;
    }

    let endpoint = normalise_endpoint(&path);
    let _timer = ScopedTimer::http_request(method.clone(), endpoint.clone());

    let response = next.run(request).await;

    let status = response.status().as_u16();
    record_http_request(&method, &endpoint, status);

    response
}

/// Spawn a background task that periodically collects storage gauges.
pub fn spawn_gauge_collector(storage: std::sync::Arc<crate::storage::Storage>) {
    tokio::spawn(async move {
        // Short delay to let startup complete
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        collect_storage_gauges(&storage).await;

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            collect_storage_gauges(&storage).await;
        }
    });
}
