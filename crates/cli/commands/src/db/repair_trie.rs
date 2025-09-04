use clap::Parser;
use reth_db::transaction::DbTx;
use reth_db_api::database::Database;
use reth_node_builder::NodeTypesWithDB;
use reth_provider::ProviderFactory;
use reth_trie::verify::Verifier;
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use tracing::info;

/// The arguments for the `reth db repair-trie` command
#[derive(Parser, Debug)]
pub struct Command {}

impl Command {
    /// Execute `db repair-trie` command
    pub fn execute<N: NodeTypesWithDB>(
        self,
        provider_factory: ProviderFactory<N>,
    ) -> eyre::Result<()> {
        // Get a database transaction directly from the database
        let db = provider_factory.db_ref();
        let mut tx = db.tx()?;
        tx.disable_long_read_transaction_safety();

        // Create the hashed cursor factory
        let hashed_cursor_factory = DatabaseHashedCursorFactory::new(&tx);

        // Create the trie cursor factory
        let trie_cursor_factory = DatabaseTrieCursorFactory::new(&tx);

        // Create the verifier
        let verifier = Verifier::new(trie_cursor_factory, hashed_cursor_factory)?;

        // Iterate over the verifier and output inconsistencies
        for inconsistency_result in verifier {
            let inconsistency = inconsistency_result?;
            info!("Inconsistency found: {inconsistency:?}");
        }

        Ok(())
    }
}

