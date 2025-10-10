//! Block announcement abstraction for outbound block propagation.

use alloy_primitives::B256;
use reth_eth_wire::NewBlock;
use std::task::{Context, Poll};

/// Strategy for how to propagate blocks to peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PropagationStrategy {
    /// Announce full block to √n peers (fast propagation to subset)
    FullBlock,
    /// Announce hash to all peers (network-wide awareness)
    HashOnly,
    /// Standard Pre-Merge Ethereum: full block to √n + hash to all
    #[default]
    Both,
}

/// Abstraction over block announcement to the network.
///
/// This trait provides the symmetric counterpart to [`BlockImport`](crate::import::BlockImport).
/// While `BlockImport` handles incoming blocks from peers, `BlockAnnounce` handles outgoing block
/// announcements to peers.
///
/// This is primarily useful for:
/// - Custom chains where the execution layer mines blocks
/// - Development/testing scenarios with custom block production
/// - Sidechains or Layer 2 solutions that sequence blocks at the execution layer
///
/// For Proof-of-Stake chains (post-merge Ethereum), this should remain unused as block
/// propagation over devp2p is invalid per [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p).
///
/// # Example
///
/// ```rust,ignore
/// use reth_network::announce::{BlockAnnounce, BlockAnnounceRequest, PropagationStrategy};
/// use tokio::sync::mpsc;
/// use std::task::{Context, Poll};
///
/// #[derive(Debug)]
/// struct ChannelBlockAnnounce {
///     rx: mpsc::UnboundedReceiver<(Block, B256)>,
/// }
///
/// impl BlockAnnounce<Block> for ChannelBlockAnnounce {
///     fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BlockAnnounceRequest<Block>> {
///         match self.rx.poll_recv(cx) {
///             Poll::Ready(Some((block, hash))) => {
///                 Poll::Ready(BlockAnnounceRequest::Announce {
///                     block,
///                     hash,
///                     strategy: PropagationStrategy::Both, // Full block + hash
///                 })
///             }
///             _ => Poll::Pending,
///         }
///     }
/// }
///
/// // Usage:
/// let (tx, rx) = mpsc::unbounded_channel();
/// let announcer = Box::new(ChannelBlockAnnounce { rx });
///
/// let config = NetworkConfig::builder(secret_key)
///     .block_announce(announcer)
///     .build(client);
///
/// // When you produce a block:
/// tx.send((new_block, block_hash)).unwrap();
/// // NetworkManager will poll and announce it automatically!
/// ```
pub trait BlockAnnounce<B = NewBlock>: std::fmt::Debug + Send + Sync {
    /// Poll for blocks that need to be announced to peers.
    ///
    /// This is called by the [`NetworkManager`](crate::NetworkManager) to check if there are any
    /// blocks ready to be announced to the network.
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BlockAnnounceRequest<B>>;
}

/// Request from block announce polling.
#[derive(Debug)]
pub enum BlockAnnounceRequest<B = NewBlock> {
    /// Block ready to announce to peers
    Announce {
        /// The block to announce
        block: B,
        /// Hash of the block
        hash: B256,
        /// How to propagate this block
        strategy: PropagationStrategy,
    },
}

/// A no-op implementation of [`BlockAnnounce`] used in Proof-of-Stake consensus.
///
/// Block propagation over devp2p is invalid in `PoS`: [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p)
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ProofOfStakeBlockAnnounce;

impl<B> BlockAnnounce<B> for ProofOfStakeBlockAnnounce {
    fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<BlockAnnounceRequest<B>> {
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use std::{
        collections::VecDeque,
        task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
    };

    /// Mock block type for testing
    #[derive(Debug, Clone)]
    struct MockBlock {
        number: u64,
        hash: B256,
    }

    unsafe fn clone_waker(_: *const ()) -> RawWaker {
        noop_raw_waker()
    }
    unsafe fn wake(_: *const ()) {}
    unsafe fn wake_by_ref(_: *const ()) {}
    unsafe fn drop_waker(_: *const ()) {}

    const NOOP_WAKER_VTABLE: RawWakerVTable =
        RawWakerVTable::new(clone_waker, wake, wake_by_ref, drop_waker);

    const fn noop_raw_waker() -> RawWaker {
        RawWaker::new(std::ptr::null(), &NOOP_WAKER_VTABLE)
    }

    /// Creates a noop waker for testing
    fn create_waker() -> Waker {
        unsafe { Waker::from_raw(noop_raw_waker()) }
    }

    /// Creates a test context with a noop waker
    fn with_context<F, R>(f: F) -> R
    where
        F: FnOnce(&mut Context<'_>) -> R,
    {
        let waker = create_waker();
        let mut cx = Context::from_waker(&waker);
        f(&mut cx)
    }

    /// Mock `BlockAnnounce` implementation for testing
    #[derive(Debug)]
    struct MockBlockAnnounce {
        blocks: VecDeque<(MockBlock, B256)>,
    }

    impl MockBlockAnnounce {
        fn new() -> Self {
            Self { blocks: VecDeque::new() }
        }

        fn add_block(&mut self, block: MockBlock, hash: B256) {
            self.blocks.push_back((block, hash));
        }
    }

    impl BlockAnnounce<MockBlock> for MockBlockAnnounce {
        fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<BlockAnnounceRequest<MockBlock>> {
            if let Some((block, hash)) = self.blocks.pop_front() {
                Poll::Ready(BlockAnnounceRequest::Announce {
                    block,
                    hash,
                    strategy: PropagationStrategy::Both,
                })
            } else {
                Poll::Pending
            }
        }
    }

    #[test]
    fn test_proof_of_stake_block_announce_always_pending() {
        let mut announcer = ProofOfStakeBlockAnnounce;

        // Should always return Pending
        with_context(|cx| {
            assert!(matches!(
                <ProofOfStakeBlockAnnounce as BlockAnnounce<MockBlock>>::poll(&mut announcer, cx),
                Poll::Pending
            ));
        });

        with_context(|cx| {
            assert!(matches!(
                <ProofOfStakeBlockAnnounce as BlockAnnounce<MockBlock>>::poll(&mut announcer, cx),
                Poll::Pending
            ));
        });
    }

    #[test]
    fn test_mock_block_announce_returns_blocks() {
        let mut announcer = MockBlockAnnounce::new();

        // Initially should be pending
        with_context(|cx| {
            assert!(matches!(announcer.poll(cx), Poll::Pending));
        });

        // Add a block
        let block = MockBlock { number: 1, hash: B256::random() };
        let hash = block.hash;
        announcer.add_block(block, hash);

        // Should now return the block
        with_context(|cx| match announcer.poll(cx) {
            Poll::Ready(BlockAnnounceRequest::Announce { block: b, hash: h, strategy }) => {
                assert_eq!(b.number, 1);
                assert_eq!(h, hash);
                assert_eq!(strategy, PropagationStrategy::Both);
            }
            Poll::Pending => panic!("Expected Ready, got Pending"),
        });

        // Should be pending again
        with_context(|cx| {
            assert!(matches!(announcer.poll(cx), Poll::Pending));
        });
    }

    #[test]
    fn test_mock_block_announce_multiple_blocks() {
        let mut announcer = MockBlockAnnounce::new();

        // Add multiple blocks
        let block1 = MockBlock { number: 1, hash: B256::random() };
        let block2 = MockBlock { number: 2, hash: B256::random() };
        let block3 = MockBlock { number: 3, hash: B256::random() };

        announcer.add_block(block1.clone(), block1.hash);
        announcer.add_block(block2.clone(), block2.hash);
        announcer.add_block(block3.clone(), block3.hash);

        // Poll should return blocks in FIFO order
        with_context(|cx| match announcer.poll(cx) {
            Poll::Ready(BlockAnnounceRequest::Announce { block, .. }) => {
                assert_eq!(block.number, 1);
            }
            Poll::Pending => panic!("Expected Ready"),
        });

        with_context(|cx| match announcer.poll(cx) {
            Poll::Ready(BlockAnnounceRequest::Announce { block, .. }) => {
                assert_eq!(block.number, 2);
            }
            Poll::Pending => panic!("Expected Ready"),
        });

        with_context(|cx| match announcer.poll(cx) {
            Poll::Ready(BlockAnnounceRequest::Announce { block, .. }) => {
                assert_eq!(block.number, 3);
            }
            Poll::Pending => panic!("Expected Ready"),
        });

        // All blocks consumed, should be pending
        with_context(|cx| {
            assert!(matches!(announcer.poll(cx), Poll::Pending));
        });
    }

    #[test]
    fn test_block_announce_event_structure() {
        let block = MockBlock { number: 42, hash: B256::random() };
        let hash = B256::random();

        let event = BlockAnnounceRequest::Announce {
            block,
            hash,
            strategy: PropagationStrategy::FullBlock,
        };

        match event {
            BlockAnnounceRequest::Announce { block: b, hash: h, strategy } => {
                assert_eq!(b.number, 42);
                assert_eq!(h, hash);
                assert_eq!(strategy, PropagationStrategy::FullBlock);
            }
        }
    }
}
