use alloy_serde::OtherFields;

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
}

impl MantleGenesisInfo {
    /// Extract the Optimism-specific genesis info from a genesis file.
    pub(crate) fn _extract_from(others: &OtherFields) -> Option<Self> {
        Self::try_from(others).ok()
    }
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
        let mantle_skadi_time = others
            .get_deserialized("mantleSkadiTime")
            .transpose()?;

        let mantle_limb_time = others
            .get_deserialized("mantleLimbTime")
            .transpose()?;

        Ok(Self { mantle_skadi_time, mantle_limb_time })
    }
}
