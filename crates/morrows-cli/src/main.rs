mod client;
mod memory;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "morrows", about = "Morrows employee tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read and edit project knowledge as ordinary local text files.
    Memory {
        #[command(subcommand)]
        command: memory::Command,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Memory { command } => memory::run(command).await,
    }
}
