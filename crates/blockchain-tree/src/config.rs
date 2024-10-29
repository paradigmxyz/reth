//! Blockchain tree configuration

/// The configuration for the blockchain tree.
#[derive(Clone, Copy, Debug)]
pub struct BlockchainTreeConfig {
    /// Number of blocks after the last finalized block that we are storing.
    ///
    /// It should be more than the finalization window for the canonical chain.
    max_blocks_in_chain: u64,
    /// The number of blocks that can be re-orged (finalization windows)
    max_reorg_depth: u64,
    /// The number of unconnected blocks that we are buffering
    max_unconnected_blocks: u32,
    /// Number of additional block hashes to save in blockchain tree. For `BLOCKHASH` EVM opcode we
    /// need last 256 block hashes.
    ///
    /// The total number of block hashes retained in-memory will be
    /// `max(additional_canonical_block_hashes, max_reorg_depth)`, and for Ethereum that would
    /// be 256. It covers both number of blocks required for reorg, and number of blocks
    /// required for `BLOCKHASH` EVM opcode.
    num_of_additional_canonical_block_hashes: u64,
}

impl Default for BlockchainTreeConfig {
    fn default() -> Self {
        // The defaults for Ethereum mainnet
        Self {
            // Gasper allows reorgs of any length from 1 to 64.
            max_reorg_depth: 64,
            // This default is just an assumption. Has to be greater than the `max_reorg_depth`.
            max_blocks_in_chain: 65,
            // EVM requires that last 256 block hashes are available.
            num_of_additional_canonical_block_hashes: 256,
            // max unconnected blocks.
            max_unconnected_blocks: 200,
        }
    }
}

impl BlockchainTreeConfig {
    /// Create tree configuration.
    pub fn new(
        max_reorg_depth: u64,
        max_blocks_in_chain: u64,
        num_of_additional_canonical_block_hashes: u64,
        max_unconnected_blocks: u32,
    ) -> Self {
        assert!(
            max_reorg_depth <= max_blocks_in_chain,
            "Side chain size should be more than finalization window"
        );
        Self {
            max_blocks_in_chain,
            max_reorg_depth,
            num_of_additional_canonical_block_hashes,
            max_unconnected_blocks,
        }
    }

    /// Return the maximum reorg depth.
    pub const fn max_reorg_depth(&self) -> u64 {
        self.max_reorg_depth
    }

    /// Return the maximum number of blocks in one chain.
    pub const fn max_blocks_in_chain(&self) -> u64 {
        self.max_blocks_in_chain
    }

    /// Return number of additional canonical block hashes that we need to retain
    /// in order to have enough information for EVM execution.
    pub const fn num_of_additional_canonical_block_hashes(&self) -> u64 {
        self.num_of_additional_canonical_block_hashes
    }

    /// Return total number of canonical hashes that we need to retain in order to have enough
    /// information for reorg and EVM execution.
    ///
    /// It is calculated as the maximum of `max_reorg_depth` (which is the number of blocks required
    /// for the deepest reorg possible according to the consensus protocol) and
    /// `num_of_additional_canonical_block_hashes` (which is the number of block hashes needed to
    /// satisfy the `BLOCKHASH` opcode in the EVM. See [`crate::BundleStateDataRef`]).
    pub fn num_of_canonical_hashes(&self) -> u64 {
        self.max_reorg_depth.max(self.num_of_additional_canonical_block_hashes)
    }

    /// Return max number of unconnected blocks that we are buffering
    pub const fn max_unconnected_blocks(&self) -> u32 {
        self.max_unconnected_blocks
    }
}
