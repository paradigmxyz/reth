use crate::{version::ParseVersionError, EthVersion};

/// This represents a shared capability, its version, and its offset.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SharedCapability {
    /// The `eth` capability.
    Eth { version: EthVersion, offset: u8 },

    /// An unknown capability.
    UnknownCapability { name: String, version: u8, offset: u8 },
}

impl SharedCapability {
    /// Creates a new [`SharedCapability`] based on the given name, offset, and version.
    pub(crate) fn new(name: &str, version: u8, offset: u8) -> Result<Self, SharedCapabilityError> {
        match name {
            "eth" => Ok(Self::Eth { version: EthVersion::try_from(version)?, offset }),
            _ => Ok(Self::UnknownCapability { name: name.to_string(), version, offset }),
        }
    }

    /// Returns the name of the capability.
    pub(crate) fn name(&self) -> &str {
        match self {
            SharedCapability::Eth { .. } => "eth",
            SharedCapability::UnknownCapability { name, .. } => name,
        }
    }

    /// Returns the version of the capability.
    pub(crate) fn version(&self) -> u8 {
        match self {
            SharedCapability::Eth { version, .. } => *version as u8,
            SharedCapability::UnknownCapability { version, .. } => *version,
        }
    }

    /// Returns the message ID offset of the current capability.
    pub(crate) fn offset(&self) -> u8 {
        match self {
            SharedCapability::Eth { offset, .. } => *offset,
            SharedCapability::UnknownCapability { offset, .. } => *offset,
        }
    }

    /// Returns the number of protocol messages supported by this capability.
    pub(crate) fn num_messages(&self) -> Result<u8, SharedCapabilityError> {
        match self {
            SharedCapability::Eth { version, .. } => Ok(version.total_messages()),
            _ => Err(SharedCapabilityError::UnknownCapability),
        }
    }
}

/// An error that may occur while creating a [`SharedCapability`].
#[derive(Debug, thiserror::Error)]
pub enum SharedCapabilityError {
    /// Unsupported `eth` version.
    #[error(transparent)]
    UnsupportedVersion(#[from] ParseVersionError),
    /// Cannot determine the number of messages for unknown capabilities.
    #[error("cannot determine the number of messages for unknown capabilities")]
    UnknownCapability,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_eth_67() {
        let capability = SharedCapability::new("eth", 67, 0).unwrap();

        assert_eq!(capability.name(), "eth");
        assert_eq!(capability.version(), 67);
        assert_eq!(capability, SharedCapability::Eth { version: EthVersion::Eth67, offset: 0 });
    }

    #[test]
    fn from_eth_66() {
        let capability = SharedCapability::new("eth", 66, 0).unwrap();

        assert_eq!(capability.name(), "eth");
        assert_eq!(capability.version(), 66);
        assert_eq!(capability, SharedCapability::Eth { version: EthVersion::Eth66, offset: 0 });
    }
}
