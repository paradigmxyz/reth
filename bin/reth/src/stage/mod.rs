//! Main `stage` command
//!
//! Stage debugging tool
use crate::{
    config::Config,
    dirs::{ConfigPath, DbPath},
    prometheus_exporter,
    util::{
        chainspec::{chain_spec_value_parser, ChainSpecification, Genesis},
        init::init_db,
    },
};
use reth_db::{
    database::Database,
    mdbx::{Env, WriteMap},
    transaction::{DbTx, DbTxMut},
};
use reth_executor::Config as ExecutorConfig;
use reth_stages::{
    metrics::HeaderMetrics, stages::sender_recovery::SenderRecoveryStage, ExecInput, Stage,
    Transaction,
};

use clap::Parser;
use serde::Deserialize;
use std::net::SocketAddr;
use strum::{AsRefStr, EnumString, EnumVariantNames};
use tracing::*;

/// `reth stage` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the database folder.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/db` or `$HOME/.local/share/reth/db`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/db`
    /// - macOS: `$HOME/Library/Application Support/reth/db`
    #[arg(long, value_name = "PATH", verbatim_doc_comment, default_value_t)]
    db: DbPath,

    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment, default_value_t)]
    config: ConfigPath,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    ///
    /// Built-in chains:
    /// - mainnet
    /// - goerli
    /// - sepolia
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = chain_spec_value_parser
    )]
    chain: ChainSpecification,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[clap(long, value_name = "SOCKET")]
    metrics: Option<SocketAddr>,

    /// The name of the stage to run
    #[arg(long, short)]
    stage: StageEnum,

    /// The height to start at
    #[arg(long)]
    from: u64,

    /// The end of the stage
    #[arg(long, short)]
    to: u64,

    /// Whether to unwind or run the stage forward
    #[arg(long, short)]
    unwind: bool,
}

#[derive(
    Debug, Clone, Copy, Eq, PartialEq, AsRefStr, EnumVariantNames, EnumString, Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "kebab-case")]
enum StageEnum {
    Headers,
    Bodies,
    Senders,
    Execution,
}

impl Command {
    /// Execute `stage` command
    pub async fn execute(&self) -> eyre::Result<()> {
        // Raise the fd limit of the process.
        // Does not do anything on windows.
        fdlimit::raise_fd_limit();

        if let Some(listen_addr) = self.metrics {
            info!("Starting metrics endpoint at {}", listen_addr);
            prometheus_exporter::initialize(listen_addr)?;
            HeaderMetrics::describe();
        }

        let config: Config = confy::load_path(&self.config).unwrap_or_default();
        info!("reth {} starting stage {:?}", clap::crate_version!(), self.stage);

        match self.stage {
            StageEnum::Senders => {
                let mut stage = SenderRecoveryStage {
                    batch_size: config.stages.sender_recovery.batch_size,
                    commit_threshold: config.stages.sender_recovery.commit_threshold,
                };
                let db = init_db(&self.db)?;
                let id = Stage::<Env<WriteMap>>::id(&stage);
                let progress = db.view(|tx| id.get_progress(tx))??.unwrap_or_default();

                // TODO: Figure out how to set start/end block range to run
                let input = ExecInput { previous_stage: None, stage_progress: None };

                let mut tx = Transaction::new(&db)?;
                stage.execute(&mut tx, input).await?;
            }
            StageEnum::Execution => {
                // let stage = ExecutionStage { config: ExecutorConfig::new_ethereum() };
            }
            _ => {}
        }

        Ok(())
    }
}
