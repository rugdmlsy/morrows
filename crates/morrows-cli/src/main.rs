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
    /// Approve a browser login from the service owner's trusted local/SSH shell.
    OperatorApprove {
        #[arg(long)]
        code: String,
        /// Existing service database. This command never creates a new database.
        #[arg(long)]
        database: std::path::PathBuf,
        #[arg(long, default_value = "operator", value_parser = ["viewer", "operator", "admin"])]
        role: String,
        #[arg(long, default_value_t = 86400)]
        ttl_seconds: i64,
    },
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
        Command::OperatorApprove {
            code,
            database,
            role,
            ttl_seconds,
        } => {
            let path = database.canonicalize()?;
            anyhow::ensure!(
                path.is_file(),
                "database must be an existing service database"
            );
            let store =
                morrows_store::Store::connect(&format!("sqlite://{}?mode=rw", path.display()))
                    .await?;
            let credential = store
                .approve_operator_login(&code, &role, ttl_seconds)
                .await?;
            println!("{}", serde_json::to_string_pretty(&credential)?);
            Ok(())
        }
    }
}
