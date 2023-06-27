use crate::BlockNumber;
use bytes::{Buf, BufMut};
use reth_codecs::{derive_arbitrary, Compact};
use serde::{Deserialize, Serialize};

/// Prune mode.
#[derive_arbitrary(compact)]
#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PruneMode {
    /// Prune all blocks.
    Full,
    /// Prune blocks before the `head-N` block number. In other words, keep last N blocks.
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

impl Compact for PruneMode {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        match self {
            PruneMode::Full => {
                buf.put_u8(0);
                1
            }
            PruneMode::Distance(distance) => {
                buf.put_u8(1);
                buf.put_u64(distance);
                1 + 8
            }
            PruneMode::Before(block_number) => {
                buf.put_u8(2);
                buf.put_u64(block_number);
                1 + 8
            }
        }
    }

    fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8])
    where
        Self: Sized,
    {
        match buf.get_u8() {
            0 => (Self::Full, buf),
            1 => (Self::Distance(buf.get_u64()), buf),
            2 => (Self::Before(buf.get_u64()), buf),
            _ => unreachable!("Junk data in database: unknown PruneMode variant"),
        }
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
