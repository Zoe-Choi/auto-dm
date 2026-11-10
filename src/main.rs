//! Instagram comment keyword to DM automation. Rust port of the Python `auto_dm` package.
//!
//! Commands:
//! - `web`: admin page and webhook server
//! - `worker`: sends queued DMs in a loop
//! - `process-once`, `init-db`, `hash-password`: one-off helpers

mod admin;
mod config;
mod db;
mod instagram;
mod security;
mod services;
mod web;
mod webhook;
mod worker;

use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::config::Settings;
use crate::instagram::GraphClient;

#[derive(Parser)]
#[command(name = "auto-dm", about = "Instagram comment keyword to DM automation")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create or migrate the local database
    InitDb,
    /// Run the admin page and webhook server
    Web {
        #[arg(long, default_value = "127.0.0.1:8000")]
        addr: String,
    },
    /// Process one batch of queued DM jobs
    ProcessOnce {
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Continuously process queued DM jobs
    Worker {
        #[arg(long, default_value_t = 50)]
        limit: i64,
        /// Seconds to sleep when the queue is empty
        #[arg(long, default_value_t = 5.0)]
        interval: f64,
    },
    /// Print the ADMIN_PASSWORD_HASH value for a password
    HashPassword { password: String },
}

async fn run_batch(settings: &Settings, client: &GraphClient, limit: i64) -> Result<usize> {
    let conn = db::open(&settings.database_path)?;
    worker::process_jobs(&conn, client, limit).await
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    let settings = Settings::from_env();

    match cli.command {
        Command::InitDb => db::init_db(&settings.database_path)?,
        Command::Web { addr } => web::serve(settings, &addr).await?,
        Command::ProcessOnce { limit } => {
            let client = GraphClient::new(settings.clone())?;
            println!("{}", run_batch(&settings, &client, limit).await?);
        }
        Command::Worker { limit, interval } => {
            db::init_db(&settings.database_path)?;
            let client = GraphClient::new(settings.clone())?;
            loop {
                // A failed batch is logged and retried instead of stopping the worker.
                let processed = run_batch(&settings, &client, limit)
                    .await
                    .unwrap_or_else(|err| {
                        tracing::error!("worker batch failed: {err:#}");
                        0
                    });
                if processed == 0 {
                    tokio::time::sleep(Duration::from_secs_f64(interval)).await;
                }
            }
        }
        Command::HashPassword { password } => println!("{}", admin::hash_admin_password(&password)),
    }
    Ok(())
}
