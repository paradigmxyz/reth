use alloy_primitives::{hex, B256};
use clap::Parser;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_node_builder::NodeTypesWithDB;
use reth_provider::{providers::ProviderNodeTypes, ProviderFactory, StageCheckpointReader};
use reth_stages::StageId;
use reth_trie::{
    verify::{Output, Verifier},
    Nibbles,
};
use reth_trie_common::{
    BranchNodeCompact, StorageTrieEntry, StoredNibbles, StoredNibblesSubKey, TrieMask,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use std::time::{Duration, Instant};
use tracing::{info, warn};

const PROGRESS_PERIOD: Duration = Duration::from_secs(5);

/// The arguments for the `reth db repair-trie` command
#[derive(Parser, Debug)]
pub struct Command {
    /// Only show inconsistencies without making any repairs
    #[arg(long)]
    pub(crate) dry_run: bool,
}

impl Command {
    /// Execute `db repair-trie` command
    pub fn execute<N: ProviderNodeTypes>(
        self,
        provider_factory: ProviderFactory<N>,
    ) -> eyre::Result<()> {
        if self.dry_run {
            verify_only(provider_factory)?
        } else {
            verify_and_repair(provider_factory)?
        }

        Ok(())
    }
}

fn verify_only<N: NodeTypesWithDB>(provider_factory: ProviderFactory<N>) -> eyre::Result<()> {
    // Get a database transaction directly from the database
    let db = provider_factory.db_ref();
    let mut tx = db.tx()?;
    tx.disable_long_read_transaction_safety();

    // Create the verifier
    let hashed_cursor_factory = DatabaseHashedCursorFactory::new(&tx);
    let trie_cursor_factory = DatabaseTrieCursorFactory::new(&tx);
    let verifier = Verifier::new(trie_cursor_factory, hashed_cursor_factory)?;

    let mut inconsistent_nodes = 0;
    let start_time = Instant::now();
    let mut last_progress_time = Instant::now();

    // Iterate over the verifier and repair inconsistencies
    for output_result in verifier {
        let output = output_result?;
        if let Output::Progress(path) = output {
            if last_progress_time.elapsed() > PROGRESS_PERIOD {
                output_progress(path, start_time, inconsistent_nodes);
                last_progress_time = Instant::now();
            }
        } else {
            warn!("Inconsistency found: {output:?}");
            inconsistent_nodes += 1;
        }
    }

    info!("Found {} inconsistencies (dry run - no changes made)", inconsistent_nodes);

    Ok(())
}

/// Checks that the merkle stage has completed running up to the account and storage hashing stages.
fn verify_checkpoints(provider: impl StageCheckpointReader) -> eyre::Result<()> {
    let account_hashing_checkpoint =
        provider.get_stage_checkpoint(StageId::AccountHashing)?.unwrap_or_default();
    let storage_hashing_checkpoint =
        provider.get_stage_checkpoint(StageId::StorageHashing)?.unwrap_or_default();
    let merkle_checkpoint =
        provider.get_stage_checkpoint(StageId::MerkleExecute)?.unwrap_or_default();

    if account_hashing_checkpoint.block_number != merkle_checkpoint.block_number {
        return Err(eyre::eyre!(
            "MerkleExecute stage checkpoint ({}) != AccountHashing stage checkpoint ({}), you must first complete the pipeline sync by running `reth node`",
            merkle_checkpoint.block_number,
            account_hashing_checkpoint.block_number,
        ))
    }

    if storage_hashing_checkpoint.block_number != merkle_checkpoint.block_number {
        return Err(eyre::eyre!(
            "MerkleExecute stage checkpoint ({}) != StorageHashing stage checkpoint ({}), you must first complete the pipeline sync by running `reth node`",
            merkle_checkpoint.block_number,
            storage_hashing_checkpoint.block_number,
        ))
    }

    let merkle_checkpoint_progress =
        provider.get_stage_checkpoint_progress(StageId::MerkleExecute)?;
    if merkle_checkpoint_progress.is_some_and(|progress| !progress.is_empty()) {
        return Err(eyre::eyre!(
            "MerkleExecute sync stage in-progress, you must first complete the pipeline sync by running `reth node`",
        ))
    }

    Ok(())
}

fn verify_and_repair<N: ProviderNodeTypes>(
    provider_factory: ProviderFactory<N>,
) -> eyre::Result<()> {
    // Get a read-write database provider
    let mut provider_rw = provider_factory.provider_rw()?;

    // Check that a pipeline sync isn't in progress.
    verify_checkpoints(provider_rw.as_ref())?;

    // Create cursors for making modifications with
    let tx = provider_rw.tx_mut();
    tx.disable_long_read_transaction_safety();
    let mut account_trie_cursor = tx.cursor_write::<tables::AccountsTrie>()?;
    let mut storage_trie_cursor = tx.cursor_dup_write::<tables::StoragesTrie>()?;

    let account =
        B256::from(hex!("0xbe67e4c7cd9e0722c7a175135f72c02b5ec1ddba0f73dacde8e007defbe9fca9"));
    let nibbles = StoredNibblesSubKey(Nibbles::from_nibbles([0x4, 0xf, 0x8, 0x8, 0x0, 0x7]));

    {
        println!("UPSERTING");
        let mut storage_trie_cursor = tx.cursor_dup_write::<tables::StoragesTrie>()?;

        // Write the two storage trie nodes as specified
        let account =
            B256::from(hex!("0xbe67e4c7cd9e0722c7a175135f72c02b5ec1ddba0f73dacde8e007defbe9fca9"));

        // Node a)
        let node_a = BranchNodeCompact::new(
            TrieMask::new(0b1111111111111111),
            TrieMask::new(0b0011110100100001),
            TrieMask::new(0b1111111111111011),
            vec![
                hex!("0xdede882d52f0e0eddfb5b89293a10c87468b4a73acd0d4ae550054a92353f6d5").into(),
                hex!("0x8746f18e465e2eed16117306b6f2eef30bc9d2978aee4a7838255e39c41a3222").into(),
                hex!("0x47d5c2bf9eea5c1ee027e4740c2b86159074a27d52fd2f6a8a8c86c77e48006f").into(),
                hex!("0xeb76a359b216e1d86b1f2803692a9fe8c3d3f97a9fe6a82b396e30344febc0c1").into(),
                hex!("0x437656f2697f167b23e33cb94acc8550128cfd647fc1579d61e982cb7616b8bc").into(),
                hex!("0x45a1ac2faf15ea8a4da6f921475974e0379f39c3d08166242255a567fa88ce6c").into(),
                hex!("0x7dbb299d714d3dfa593f53bc1b8c66d5c401c30a0b5587b01254a56330361395").into(),
                hex!("0xae407eb14a74ed951c9949c1867fb9ee9ba5d5b7e03769eaf3f29c687d080429").into(),
                hex!("0x768d0fe1003f0e85d3bc76e4a1fa0827f63b10ca9bca52d56c2b1cceb8eb8b08").into(),
                hex!("0xe5127935143493d5094f4da6e4f7f5a0f62d524fbb61e7bb9fb63d8a166db0f3").into(),
                hex!("0x7f3698297308664fbc1b9e2c41d097fbd57d8f364c394f6ad7c71b10291fbf42").into(),
                hex!("0x4a2bc7e19cec63cb5ef5754add0208959b50bcc79f13a22a370f77b277dbe6db").into(),
                hex!("0x40764b8c48de59258e62a3371909a107e76e1b5e847cfa94dbc857e9fd205103").into(),
                hex!("0x2985dca29a7616920d95c43ab62eb013a40e6a0c88c284471e4c3bd22f3b9b25").into(),
                hex!("0x1b6511f7a385e79477239f7dd4a49f52082ecac05aa5bd0de18b1d55fe69d10c").into(),
            ],
            None,
        );
        let nibbles_a = StoredNibblesSubKey(Nibbles::from_nibbles([0x4, 0xf, 0x8, 0x8, 0x0, 0x7]));
        let entry_a = StorageTrieEntry { nibbles: nibbles_a.clone(), node: node_a };
        storage_trie_cursor.upsert(account, &entry_a)?;
    }

    {
        println!("WALKING");
        let mut storage_trie_cursor = tx.cursor_dup_write::<tables::StoragesTrie>()?;
        let mut walker = storage_trie_cursor.walk_dup(Some(account), Some(nibbles.clone()))?;

        let mut prev = None;
        while let Some((_, entry)) = walker.next().transpose()? {
            println!("{entry:?}");
            if prev.is_some_and(|prev| prev != entry.nibbles.clone()) {
                break;
            }

            prev = Some(entry.nibbles.clone());

            if !entry.nibbles.starts_with(&nibbles) {
                break;
            }
        }
    }

    // Create the cursor factories. These cannot accept the `&mut` tx above because they require it
    // to be AsRef.
    let tx = provider_rw.tx_ref();
    let hashed_cursor_factory = DatabaseHashedCursorFactory::new(tx);
    let trie_cursor_factory = DatabaseTrieCursorFactory::new(tx);

    // Create the verifier
    let verifier = Verifier::new(trie_cursor_factory, hashed_cursor_factory)?;

    let mut inconsistent_nodes = 0;
    let start_time = Instant::now();
    let mut last_progress_time = Instant::now();

    // Iterate over the verifier and repair inconsistencies
    for output_result in verifier {
        let output = output_result?;

        if !matches!(output, Output::Progress(_)) {
            warn!("Inconsistency found, will repair: {output:?}");
            inconsistent_nodes += 1;
        }

        match output {
            Output::AccountExtra(path, _node) => {
                // Extra account node in trie, remove it
                let nibbles = StoredNibbles(path);
                if account_trie_cursor.seek_exact(nibbles)?.is_some() {
                    account_trie_cursor.delete_current()?;
                }
            }
            Output::StorageExtra(account, path, _node) => {
                // Extra storage node in trie, remove it
                let nibbles = StoredNibblesSubKey(path);
                if storage_trie_cursor
                    .seek_by_key_subkey(account, nibbles.clone())?
                    .filter(|e| e.nibbles == nibbles)
                    .is_some()
                {
                    storage_trie_cursor.delete_current()?;
                }
            }
            Output::AccountWrong { path, expected: node, .. } |
            Output::AccountMissing(path, node) => {
                // Wrong/missing account node value, upsert it
                let nibbles = StoredNibbles(path);
                account_trie_cursor.upsert(nibbles, &node)?;
            }
            Output::StorageWrong { account, path, expected: node, .. } |
            Output::StorageMissing(account, path, node) => {
                // Wrong/missing storage node value, upsert it
                let nibbles = StoredNibblesSubKey(path);
                let entry = StorageTrieEntry { nibbles, node };
                storage_trie_cursor.upsert(account, &entry)?;
            }
            Output::Progress(path) => {
                if last_progress_time.elapsed() > PROGRESS_PERIOD {
                    output_progress(path, start_time, inconsistent_nodes);
                    last_progress_time = Instant::now();
                }
            }
        }
    }

    if inconsistent_nodes == 0 {
        info!("No inconsistencies found");
    } else {
        //info!("Repaired {} inconsistencies, committing changes", inconsistent_nodes);
        //provider_rw.commit()?;
    }

    Ok(())
}

/// Output progress information based on the last seen account path.
fn output_progress(last_account: Nibbles, start_time: Instant, inconsistent_nodes: u64) {
    // Calculate percentage based on position in the trie path space
    // For progress estimation, we'll use the first few nibbles as an approximation

    // Convert the first 16 nibbles (8 bytes) to a u64 for progress calculation
    let mut current_value: u64 = 0;
    let nibbles_to_use = last_account.len().min(16);

    for i in 0..nibbles_to_use {
        current_value = (current_value << 4) | (last_account.get(i).unwrap_or(0) as u64);
    }
    // Shift left to fill remaining bits if we have fewer than 16 nibbles
    if nibbles_to_use < 16 {
        current_value <<= (16 - nibbles_to_use) * 4;
    }

    let progress_percent = current_value as f64 / u64::MAX as f64 * 100.0;
    let progress_percent_str = format!("{progress_percent:.2}");

    // Calculate ETA based on current speed
    let elapsed = start_time.elapsed();
    let elapsed_secs = elapsed.as_secs_f64();

    let estimated_total_time =
        if progress_percent > 0.0 { elapsed_secs / (progress_percent / 100.0) } else { 0.0 };
    let remaining_time = estimated_total_time - elapsed_secs;
    let eta_duration = Duration::from_secs(remaining_time as u64);

    info!(
        progress_percent = progress_percent_str,
        eta = %humantime::format_duration(eta_duration),
        inconsistent_nodes,
        "Repairing trie tables",
    );
}
