use crate::{hardforks::Hardforks, ForkCondition};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

/// A container to pretty-print a hardfork.
///
/// The fork is formatted depending on its fork condition:
///
/// - Block and timestamp based forks are formatted in the same manner (`{name} <({eip})>
///   @{condition}`)
/// - TTD based forks are formatted separately as `{name} <({eip})> @{ttd} (network is <not> known
///   to be merged)`
///
/// An optional EIP can be attached to the fork to display as well. This should generally be in the
/// form of just `EIP-x`, e.g. `EIP-1559`.
#[derive(Debug)]
struct DisplayFork {
    /// The name of the hardfork (e.g. Frontier)
    name: String,
    /// The fork condition
    activated_at: ForkCondition,
    /// An optional EIP (e.g. `EIP-1559`).
    eip: Option<String>,
}

impl core::fmt::Display for DisplayFork {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let name_with_eip = if let Some(eip) = &self.eip {
            format!("{} ({})", self.name, eip)
        } else {
            self.name.clone()
        };

        match self.activated_at {
            ForkCondition::Block(at) | ForkCondition::Timestamp(at) => {
                write!(f, "{name_with_eip:32} @{at}")?;
            }
            ForkCondition::TTD { fork_block, total_difficulty } => {
                write!(
                    f,
                    "{:32} @{} ({})",
                    name_with_eip,
                    total_difficulty,
                    if fork_block.is_some() {
                        "network is known to be merged"
                    } else {
                        "network is not known to be merged"
                    }
                )?;
            }
            ForkCondition::Never => unreachable!(),
        }

        Ok(())
    }
}

// # Examples
//
// ```
// # use reth_chainspec::MAINNET;
// println!("{}", MAINNET.display_hardforks());
// ```
//
/// A container for pretty-printing a list of hardforks.
///
/// An example of the output:
///
/// ```text
/// Pre-merge hard forks (block based):
// - Frontier                         @0
// - Homestead                        @1150000
// - Dao                              @1920000
// - Tangerine                        @2463000
// - SpuriousDragon                   @2675000
// - Byzantium                        @4370000
// - Constantinople                   @7280000
// - Petersburg                       @7280000
// - Istanbul                         @9069000
// - MuirGlacier                      @9200000
// - Berlin                           @12244000
// - London                           @12965000
// - ArrowGlacier                     @13773000
// - GrayGlacier                      @15050000
// Merge hard forks:
// - Paris                            @58750000000000000000000 (network is known to be merged)
// Post-merge hard forks (timestamp based):
// - Shanghai                         @1681338455
// - Cancun                           @1710338135"
/// ```
#[derive(Debug)]
pub struct DisplayHardforks {
    /// A list of pre-merge (block based) hardforks
    pre_merge: Vec<DisplayFork>,
    /// A list of merge (TTD based) hardforks
    with_merge: Vec<DisplayFork>,
    /// A list of post-merge (timestamp based) hardforks
    post_merge: Vec<DisplayFork>,
}

impl core::fmt::Display for DisplayHardforks {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        fn format(
            header: &str,
            forks: &[DisplayFork],
            next_is_empty: bool,
            f: &mut core::fmt::Formatter<'_>,
        ) -> core::fmt::Result {
            writeln!(f, "{header}:")?;
            let mut iter = forks.iter().peekable();
            while let Some(fork) = iter.next() {
                write!(f, "- {fork}")?;
                if !next_is_empty || iter.peek().is_some() {
                    writeln!(f)?;
                }
            }
            Ok(())
        }

        format(
            "Pre-merge hard forks (block based)",
            &self.pre_merge,
            self.with_merge.is_empty(),
            f,
        )?;

        if !self.with_merge.is_empty() {
            format("Merge hard forks", &self.with_merge, self.post_merge.is_empty(), f)?;
        }

        if !self.post_merge.is_empty() {
            format("Post-merge hard forks (timestamp based)", &self.post_merge, true, f)?;
        }

        Ok(())
    }
}

impl DisplayHardforks {
    /// Creates a new [`DisplayHardforks`] from an iterator of hardforks.
    pub fn new<H: Hardforks>(hardforks: &H, known_paris_block: Option<u64>) -> Self {
        let mut pre_merge = Vec::new();
        let mut with_merge = Vec::new();
        let mut post_merge = Vec::new();

        for (fork, condition) in hardforks.forks_iter() {
            let mut display_fork =
                DisplayFork { name: fork.name().to_string(), activated_at: condition, eip: None };

            match condition {
                ForkCondition::Block(_) => {
                    pre_merge.push(display_fork);
                }
                ForkCondition::TTD { total_difficulty, .. } => {
                    display_fork.activated_at =
                        ForkCondition::TTD { fork_block: known_paris_block, total_difficulty };
                    with_merge.push(display_fork);
                }
                ForkCondition::Timestamp(_) => {
                    post_merge.push(display_fork);
                }
                ForkCondition::Never => continue,
            }
        }

        Self { pre_merge, with_merge, post_merge }
    }
}
