use crate::{prune::PruneMode, BlockNumber};
use reth_codecs::{main_codec, Compact};

/// Saves the pruning progress of a stage.
#[main_codec]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[cfg_attr(test, derive(Default))]
pub struct PruneCheckpoint {
    /// Prune mode.
    prune_mode: PruneMode,
    /// Highest pruned block number.
    block_number: BlockNumber,
}

#[cfg(test)]
mod tests {
    use crate::prune::{PruneCheckpoint, PruneMode};
    use rand::Rng;
    use reth_codecs::Compact;

    #[test]
    fn prune_checkpoint_roundtrip() {
        let mut rng = rand::thread_rng();
        let checkpoints = vec![
            PruneCheckpoint { prune_mode: PruneMode::Distance(rng.gen()), block_number: rng.gen() },
            PruneCheckpoint { prune_mode: PruneMode::Before(rng.gen()), block_number: rng.gen() },
        ];

        for checkpoint in checkpoints {
            let mut buf = Vec::new();
            let encoded = checkpoint.to_compact(&mut buf);
            let (decoded, _) = PruneCheckpoint::from_compact(&buf, encoded);
            assert_eq!(decoded, checkpoint);
        }
    }
}
