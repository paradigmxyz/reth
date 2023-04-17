mod database;
mod state;
pub use state::{
    historical::{HistoricalStateProvider, HistoricalStateProviderRef},
    latest::{LatestStateProvider, LatestStateProviderRef},
};
mod post_state_provider;
pub use post_state_provider::PostStateProvider;
pub use database::*;

pub struct BlockchainProvider<DB, Tree> {
    database: ShareableDatabase<DB>,
    tree: Tree
}