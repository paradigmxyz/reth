use alloc::vec;
use alloy_hardforks::Hardfork;
use alloy_serde::OtherFields;
use reth_chainspec::{BaseFeeParams, BaseFeeParamsKind};
use reth_mantle_forks::{MantleHardfork, MANTLE_MAINNET_CHAIN_ID, MANTLE_SEPOLIA_CHAIN_ID};

/// Mantle 网络特定的链信息
#[derive(Debug, Default, Clone)]
pub(crate) struct MantleChainInfo {
    /// Genesis information
    pub genesis_info: Option<MantleGenesisInfo>,
}

impl MantleChainInfo {
    /// Extracts the Optimism specific fields from a genesis file. These fields are expected to be
    /// contained in the `genesis.config` under `extra_fields` property.
    pub(crate) fn extract_from(others: &OtherFields) -> Option<Self> {
        Self::try_from(others).ok()
    }
}

impl TryFrom<&OtherFields> for MantleChainInfo {
    type Error = serde_json::Error;

    fn try_from(others: &OtherFields) -> Result<Self, Self::Error> {
        let genesis_info = MantleGenesisInfo::try_from(others).ok();

        Ok(Self { genesis_info })
    }
}

/// The Optimism-specific genesis block specification.
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub(crate) struct MantleGenesisInfo {
    /// Mantle Skadi upgrade timestamp
    pub mantle_skadi_time: Option<u64>,
    /// Mantle Limb upgrade timestamp
    pub mantle_limb_time: Option<u64>,
    /// Mantle Arsia upgrade timestamp
    pub mantle_arsia_time: Option<u64>,
}

#[cfg(feature = "serde")]
impl TryFrom<&OtherFields> for MantleGenesisInfo {
    type Error = serde_json::Error;

    fn try_from(others: &OtherFields) -> Result<Self, Self::Error> {
        others.deserialize_as()
    }
}

#[cfg(not(feature = "serde"))]
impl TryFrom<&OtherFields> for MantleGenesisInfo {
    type Error = serde_json::Error;

    fn try_from(others: &OtherFields) -> Result<Self, Self::Error> {
        let mantle_skadi_time = others.get_deserialized("mantleSkadiTime").transpose()?;
        let mantle_limb_time = others.get_deserialized("mantleLimbTime").transpose()?;
        let mantle_arsia_time = others.get_deserialized("mantleArsiaTime").transpose()?;

        Ok(Self { mantle_skadi_time, mantle_limb_time, mantle_arsia_time })
    }
}

pub(crate) fn should_use_mantle_alignment(
    chain_id: u64,
    mantle_genesis_info: Option<MantleGenesisInfo>,
) -> bool {
    let has_mantle_hardfork_config = mantle_genesis_info.is_some_and(|info| {
        info.mantle_skadi_time.is_some() ||
            info.mantle_limb_time.is_some() ||
            info.mantle_arsia_time.is_some()
    });

    has_mantle_hardfork_config ||
        matches!(chain_id, MANTLE_MAINNET_CHAIN_ID | MANTLE_SEPOLIA_CHAIN_ID)
}

pub(crate) fn extract_mantle_base_fee_params(
    optimism_chain_info: &op_alloy_rpc_types::OpChainInfo,
) -> BaseFeeParamsKind {
    // op-geth: src/mantle-v2/op-node/rollup/mantle_types.go AlignOpWithMantle() 169-181
    optimism_chain_info
        .base_fee_info
        .and_then(|info| {
            info.eip1559_denominator.zip(info.eip1559_elasticity).map(
                |(denominator, elasticity)| {
                    BaseFeeParamsKind::Variable(
                        vec![(
                            MantleHardfork::Arsia.boxed(),
                            BaseFeeParams::new(denominator as u128, elasticity as u128),
                        )]
                        .into(),
                    )
                },
            )
        })
        .unwrap_or_else(|| BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()))
}
