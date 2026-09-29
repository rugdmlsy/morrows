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
    /// Merge an OAuth-backed technical AgentInstance into a canonical AgentInstance.
    AgentMerge {
        #[arg(long)]
        source_agent_id: uuid::Uuid,
        #[arg(long)]
        target_agent_id: uuid::Uuid,
        /// Existing service database. This command never creates a new database.
        #[arg(long)]
        database: std::path::PathBuf,
        #[arg(long, default_value = "morrows-cli")]
        bound_by: String,
    },
    /// Rename one non-archived AgentInstance.
    AgentRename {
        #[arg(long)]
        agent_id: uuid::Uuid,
        #[arg(long)]
        display_name: String,
        /// Existing service database. This command never creates a new database.
        #[arg(long)]
        database: std::path::PathBuf,
    },
    /// Archive one AgentInstance without deleting historical references.
    AgentArchive {
        #[arg(long)]
        agent_id: uuid::Uuid,
        /// Existing service database. This command never creates a new database.
        #[arg(long)]
        database: std::path::PathBuf,
    },
    /// Bind a legacy unbound task to an existing Project without allowing reassignment.
    TaskBindProject {
        #[arg(long)]
        task_id: uuid::Uuid,
        #[arg(long)]
        project_id: uuid::Uuid,
        /// Existing service database. This command never creates a new database.
        #[arg(long)]
        database: std::path::PathBuf,
    },
    /// Submit a work request through the employee MCP surface.
    WorkRequest {
        /// Work request title.
        title: String,
        /// Optional description.
        #[arg(long, default_value = "")]
        description: String,
        /// Existing Project UUID. Omit to create an explicitly unbound task.
        #[arg(long)]
        project_id: Option<String>,
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
        Command::AgentMerge {
            source_agent_id,
            target_agent_id,
            database,
            bound_by,
        } => {
            let path = database.canonicalize()?;
            anyhow::ensure!(
                path.is_file(),
                "database must be an existing service database"
            );
            let store =
                morrows_store::Store::connect(&format!("sqlite://{}?mode=rw", path.display()))
                    .await?;
            let agent = store
                .merge_oauth_agent_into(source_agent_id, target_agent_id, &bound_by)
                .await?;
            println!("{}", serde_json::to_string_pretty(&agent)?);
            Ok(())
        }
        Command::AgentRename {
            agent_id,
            display_name,
            database,
        } => {
            let path = database.canonicalize()?;
            anyhow::ensure!(
                path.is_file(),
                "database must be an existing service database"
            );
            let store =
                morrows_store::Store::connect(&format!("sqlite://{}?mode=rw", path.display()))
                    .await?;
            let agent = store.rename_agent(agent_id, &display_name).await?;
            println!("{}", serde_json::to_string_pretty(&agent)?);
            Ok(())
        }
        Command::AgentArchive { agent_id, database } => {
            let path = database.canonicalize()?;
            anyhow::ensure!(
                path.is_file(),
                "database must be an existing service database"
            );
            let store =
                morrows_store::Store::connect(&format!("sqlite://{}?mode=rw", path.display()))
                    .await?;
            let agent = store.archive_agent(agent_id).await?;
            println!("{}", serde_json::to_string_pretty(&agent)?);
            Ok(())
        }
        Command::TaskBindProject {
            task_id,
            project_id,
            database,
        } => {
            let path = database.canonicalize()?;
            anyhow::ensure!(
                path.is_file(),
                "database must be an existing service database"
            );
            let store =
                morrows_store::Store::connect(&format!("sqlite://{}?mode=rw", path.display()))
                    .await?;
            let task = store.bind_unbound_task_project(task_id, project_id).await?;
            println!("{}", serde_json::to_string_pretty(&task)?);
            Ok(())
        }
        Command::WorkRequest {
            title,
            description,
            project_id,
        } => {
            let client = client::Client::connect().await?;
            let value = client
                .call(
                    "work_request_submit",
                    serde_json::json!({
                        "title": title,
                        "description": description,
                        "project_id": project_id,
                    }),
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn work_request_cli_exposes_project_id() {
        let command = Cli::command();
        let work_request = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "work-request")
            .expect("work-request subcommand");
        assert!(
            work_request
                .get_arguments()
                .any(|argument| argument.get_id().as_str() == "project_id"),
            "work-request CLI must advertise --project-id"
        );
        for (name, ids) in [
            (
                "agent-merge",
                vec!["source_agent_id", "target_agent_id", "database", "bound_by"],
            ),
            ("agent-rename", vec!["agent_id", "display_name", "database"]),
            ("agent-archive", vec!["agent_id", "database"]),
        ] {
            let subcommand = command
                .get_subcommands()
                .find(|subcommand| subcommand.get_name() == name)
                .unwrap_or_else(|| panic!("{name} subcommand"));
            for id in ids {
                assert!(
                    subcommand
                        .get_arguments()
                        .any(|argument| argument.get_id().as_str() == id),
                    "{name} must advertise {id}"
                );
            }
        }
        let repair = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "task-bind-project")
            .expect("task-bind-project subcommand");
        for id in ["task_id", "project_id", "database"] {
            assert!(
                repair
                    .get_arguments()
                    .any(|argument| argument.get_id().as_str() == id),
                "task-bind-project must advertise {id}"
            );
        }
    }
}
