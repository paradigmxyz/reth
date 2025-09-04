use clap::Parser;
use reth_db_api::database::Database;
use reth_node_builder::NodeTypesWithDB;
use reth_provider::ProviderFactory;
use reth_trie::verify::Verifier;
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};

/// The arguments for the `reth db repair-trie` command
#[derive(Parser, Debug)]
pub struct Command {
    /// Output inconsistencies as JSON, one per line
    #[arg(long)]
    json: bool,
}

impl Command {
    /// Execute `db repair-trie` command
    pub fn execute<N: NodeTypesWithDB>(
        self,
        provider_factory: ProviderFactory<N>,
    ) -> eyre::Result<()> {
        // Get a database transaction directly from the database
        let db = provider_factory.db_ref();
        let tx = db.tx()?;

        // Create the hashed cursor factory
        let hashed_cursor_factory = DatabaseHashedCursorFactory::new(&tx);

        // Create the trie cursor factory
        let trie_cursor_factory = DatabaseTrieCursorFactory::new(&tx);

        // Create the verifier
        let verifier = Verifier::new(trie_cursor_factory, hashed_cursor_factory)?;

        // Iterate over the verifier and output inconsistencies
        for inconsistency_result in verifier {
            let inconsistency = inconsistency_result?;

            if self.json {
                // Serialize to JSON
                println!("{}", serde_json::to_string(&inconsistency)?);
            } else {
                // Output as debug format
                println!("{:?}", inconsistency);
            }
        }

        Ok(())
    }
}