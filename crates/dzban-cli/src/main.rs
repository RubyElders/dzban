use std::process::ExitCode;

use dzban_cli::args::Args;

#[tokio::main]
async fn main() -> ExitCode {
    let matches = Args::grouped_command().get_matches();
    let args = Args::from_matches(&matches).unwrap_or_else(|error| error.exit());
    if args.verbose > 0 {
        let level = if args.verbose > 1 { "debug" } else { "info" };
        tracing_subscriber::fmt()
            .with_env_filter(level)
            .with_writer(std::io::stderr)
            .init();
    }
    match dzban_cli::dispatch::run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("dzb: {}", error.message);
            ExitCode::from(error.code)
        }
    }
}
