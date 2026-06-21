use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "smol-sandbox-exe-dev")]
#[command(about = "smol-workflows exe.dev sandbox provider")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Serve the smol-workflows sandbox JSONL protocol on stdin/stdout.
    Serve,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Serve => smol_sandbox_exe_dev::jsonl_server::serve_stdio().await,
    };

    if let Err(error) = result {
        eprintln!("smol-sandbox-exe-dev: {error}");
        std::process::exit(1);
    }
}
