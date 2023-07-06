use crate::BlockNumber;
use reth_codecs::{main_codec, Compact};

/// Prune mode.
#[main_codec]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum PruneMode {
    /// Prune all blocks.
    Full,
    /// Prune blocks before the `head-N` block number. In other words, keep last N + 1 blocks.
    Distance(u64),
    /// Prune blocks before the specified block number. The specified block number is not pruned.
    Before(BlockNumber),
}

#[cfg(test)]
impl Default for PruneMode {
    fn default() -> Self {
        Self::Distance(0)
    }
}

#[cfg(test)]
mod tests {
    use crate::prune::PruneMode;
    use assert_matches::assert_matches;
    use serde::Deserialize;

    #[test]
    fn prune_mode_deserialize() {
        #[derive(Debug, Deserialize)]
        struct Config {
            a: Option<PruneMode>,
            b: Option<PruneMode>,
            c: Option<PruneMode>,
            d: Option<PruneMode>,
        }

        let toml_str = r#"
        a = "full"
        b = { distance = 10 }
        c = { before = 20 }
    "#;

        assert_matches!(
            toml::from_str(toml_str),
            Ok(Config {
                a: Some(PruneMode::Full),
                b: Some(PruneMode::Distance(10)),
                c: Some(PruneMode::Before(20)),
                d: None
            })
        );
    }
}
