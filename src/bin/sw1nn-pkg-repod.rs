use clap::Parser;
use sw1nn_pkg_repo::run_service;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(name = "sw1nn-pkg-repod")]
#[command(about = "Package repository server", long_about = None)]
#[command(version = VERSION)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, value_name = "FILE")]
    config: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    run_service(args.config.as_deref()).await
}
