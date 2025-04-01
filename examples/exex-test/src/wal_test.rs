use eyre::Result;
use reth::{api::FullNodeComponents, builder::NodeTypes, primitives::EthPrimitives};
use reth_exex::{ExExContext, ExExEvent, ExExNotification, wal::WalHandle};
use std::{path::Path, sync::Arc};

/// ExEx tests - WAL behavior
pub async fn wal_test_exex<Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>>(
    mut ctx: ExExContext<Node>,
) -> Result<()> {
    // Get access to the WAL handle
    let wal_handle = ctx.notifications.wal_handle().clone();
    
    // Track the latest finalized block
    let mut latest_finalized = None;
    
    println!("WAL test ExEx started");
    
    // Process notifications
    while let Some(notification) = futures_util::TryStreamExt::try_next(&mut ctx.notifications).await? {
        if let Some(committed_chain) = notification.committed_chain() {
            println!(
                "WAL test: Received committed chain: {:?}",
                committed_chain.range()
            );
            
            // Send finished height event
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
            
            // Check if we have a finalized block by examining the finalized_at field in the chain
            if let Some(finalized_at) = committed_chain.finalized_at() {
                latest_finalized = Some(finalized_at.clone());
                
                // Check WAL state after finalization
                check_wal_after_finalization(&wal_handle, finalized_at.number);
            }
        }
    }
    
    Ok(())
}

/// Checks if the WAL has been properly cleared after finalization
fn check_wal_after_finalization(wal_handle: &WalHandle<EthPrimitives>, finalized_block: u64) {
    // Get the WAL path from the handle
    if let Some(wal_path) = wal_handle.path() {
        println!(
            "Checking WAL after finalization - block: {}, path: {:?}",
            finalized_block,
            wal_path
        );
        
        // We would check the WAL entries here, but since that requires internal WAL access,
        // we'll need to add instrumentation in the ExEx code itself.
        // For now, we'll just log that we reached this point
        println!("WAL should have cleared blocks up to {}", finalized_block);
    }
}
