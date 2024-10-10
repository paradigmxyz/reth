use std::sync::Arc;

use reth_chain_state::CanonStateNotification;
use reth_execution_types::Chain;

/// Notifications sent to an `ExEx`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ExExNotification {
    /// Chain got committed without a reorg, and only the new chain is returned.
    ChainCommitted {
        /// The new chain after commit.
        new: Arc<Chain>,
    },
    /// Chain got reorged, and both the old and the new chains are returned.
    ChainReorged {
        /// The old chain before reorg.
        old: Arc<Chain>,
        /// The new chain after reorg.
        new: Arc<Chain>,
    },
    /// Chain got reverted, and only the old chain is returned.
    ChainReverted {
        /// The old chain before reversion.
        old: Arc<Chain>,
    },
}

impl ExExNotification {
    /// Returns the committed chain from the [`Self::ChainCommitted`] and [`Self::ChainReorged`]
    /// variants, if any.
    pub fn committed_chain(&self) -> Option<Arc<Chain>> {
        match self {
            Self::ChainCommitted { new } | Self::ChainReorged { old: _, new } => Some(new.clone()),
            Self::ChainReverted { .. } => None,
        }
    }

    /// Returns the reverted chain from the [`Self::ChainReorged`] and [`Self::ChainReverted`]
    /// variants, if any.
    pub fn reverted_chain(&self) -> Option<Arc<Chain>> {
        match self {
            Self::ChainReorged { old, new: _ } | Self::ChainReverted { old } => Some(old.clone()),
            Self::ChainCommitted { .. } => None,
        }
    }

    /// Converts the notification into a notification that is the inverse of the original one.
    ///
    /// - For [`Self::ChainCommitted`], it's [`Self::ChainReverted`].
    /// - For [`Self::ChainReverted`], it's [`Self::ChainCommitted`].
    /// - For [`Self::ChainReorged`], it's [`Self::ChainReorged`] with the new chain as the old
    ///   chain and the old chain as the new chain.
    pub fn into_inverted(self) -> Self {
        match self {
            Self::ChainCommitted { new } => Self::ChainReverted { old: new },
            Self::ChainReverted { old } => Self::ChainCommitted { new: old },
            Self::ChainReorged { old, new } => Self::ChainReorged { old: new, new: old },
        }
    }
}

impl From<CanonStateNotification> for ExExNotification {
    fn from(notification: CanonStateNotification) -> Self {
        match notification {
            CanonStateNotification::Commit { new } => Self::ChainCommitted { new },
            CanonStateNotification::Reorg { old, new } => Self::ChainReorged { old, new },
        }
    }
}

/// Bincode-compatible [`ExExNotification`] serde implementation.
#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub(super) mod serde_bincode_compat {
    use std::sync::Arc;

    use reth_execution_types::serde_bincode_compat::Chain;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`super::ExExNotification`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_exex_types::{serde_bincode_compat, ExExNotification};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::ExExNotification")]
    ///     notification: ExExNotification,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    #[allow(missing_docs)]
    pub enum ExExNotification<'a> {
        ChainCommitted { new: Chain<'a> },
        ChainReorged { old: Chain<'a>, new: Chain<'a> },
        ChainReverted { old: Chain<'a> },
    }

    impl<'a> From<&'a super::ExExNotification> for ExExNotification<'a> {
        fn from(value: &'a super::ExExNotification) -> Self {
            match value {
                super::ExExNotification::ChainCommitted { new } => {
                    ExExNotification::ChainCommitted { new: Chain::from(new.as_ref()) }
                }
                super::ExExNotification::ChainReorged { old, new } => {
                    ExExNotification::ChainReorged {
                        old: Chain::from(old.as_ref()),
                        new: Chain::from(new.as_ref()),
                    }
                }
                super::ExExNotification::ChainReverted { old } => {
                    ExExNotification::ChainReverted { old: Chain::from(old.as_ref()) }
                }
            }
        }
    }

    impl<'a> From<ExExNotification<'a>> for super::ExExNotification {
        fn from(value: ExExNotification<'a>) -> Self {
            match value {
                ExExNotification::ChainCommitted { new } => {
                    Self::ChainCommitted { new: Arc::new(new.into()) }
                }
                ExExNotification::ChainReorged { old, new } => {
                    Self::ChainReorged { old: Arc::new(old.into()), new: Arc::new(new.into()) }
                }
                ExExNotification::ChainReverted { old } => {
                    Self::ChainReverted { old: Arc::new(old.into()) }
                }
            }
        }
    }

    impl SerializeAs<super::ExExNotification> for ExExNotification<'_> {
        fn serialize_as<S>(
            source: &super::ExExNotification,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            ExExNotification::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::ExExNotification> for ExExNotification<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::ExExNotification, D::Error>
        where
            D: Deserializer<'de>,
        {
            ExExNotification::deserialize(deserializer).map(Into::into)
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::Arc;

        use arbitrary::Arbitrary;
        use rand::Rng;
        use reth_execution_types::Chain;
        use reth_primitives::SealedBlockWithSenders;
        use serde::{Deserialize, Serialize};
        use serde_with::serde_as;

        use super::super::{serde_bincode_compat, ExExNotification};

        #[test]
        fn test_exex_notification_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::ExExNotification")]
                notification: ExExNotification,
            }

            let mut bytes = [0u8; 1024];
            rand::thread_rng().fill(bytes.as_mut_slice());
            let data = Data {
                notification: ExExNotification::ChainReorged {
                    old: Arc::new(Chain::new(
                        vec![SealedBlockWithSenders::arbitrary(&mut arbitrary::Unstructured::new(
                            &bytes,
                        ))
                        .unwrap()],
                        Default::default(),
                        None,
                    )),
                    new: Arc::new(Chain::new(
                        vec![SealedBlockWithSenders::arbitrary(&mut arbitrary::Unstructured::new(
                            &bytes,
                        ))
                        .unwrap()],
                        Default::default(),
                        None,
                    )),
                },
            };

            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }
    }
}
