use clap::Parser;
use ns2pro_bridge::{app, cli::Options};
#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let options = Options::parse();
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| options.log_level.clone());
    let filter = match tracing_subscriber::EnvFilter::try_new(filter) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("invalid log filter: {e}");
            return std::process::ExitCode::from(2);
        }
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    let local = tokio::task::LocalSet::new();
    match local.run_until(app::run(options)).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error=%e,hint=e.hint(),"bridge stopped");
            std::process::ExitCode::from(e.exit_code())
        }
    }
}
