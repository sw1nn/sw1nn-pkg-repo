use sw1nn_pkg_repo::run_service;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_service().await
}
