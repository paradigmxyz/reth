//! Utility methods for clique consensus.
use reth_interfaces::consensus::CliqueError;
use reth_primitives::{
    constants::clique::{EXTRA_SEAL, EXTRA_VANITY},
    recovery::secp256k1,
    Address, BlockNumber, Bytes, CliqueConfig, SealedHeader,
};
use std::collections::BTreeSet;

/// Recover the account from signed header per clique consensus rules.
pub fn recover_header_signer(header: &SealedHeader) -> Result<Address, CliqueError> {
    let extra_data_len = header.extra_data.len();
    let signature = extra_data_len
        .checked_sub(EXTRA_SEAL)
        .and_then(|start| -> Option<[u8; 65]> { header.extra_data[start..].try_into().ok() })
        .ok_or(CliqueError::MissingSignature { extra_data: header.extra_data.clone() })?;
    secp256k1::recover(&signature, header.hash().as_fixed_bytes())
        .map_err(|_| CliqueError::HeaderSignerRecovery { signature, hash: header.hash() })
}

/// Return `true` if the block is a checkpoint block.
#[inline]
pub fn is_checkpoint_block(config: &CliqueConfig, block: BlockNumber) -> bool {
    block % config.epoch == 0
}

/// Construct a vector of signers from extra data of a checkpoint header.
pub fn extract_checkpoint_signers(extra_data: Bytes) -> Result<BTreeSet<Address>, CliqueError> {
    if extra_data.len() <= EXTRA_VANITY + EXTRA_SEAL {
        return Err(CliqueError::CheckpointSigners { extra_data })
    }
    let address_len = Address::len_bytes();
    let signers_bytes_len = extra_data.len() - EXTRA_VANITY - EXTRA_SEAL;
    if signers_bytes_len % address_len != 0 {
        return Err(CliqueError::CheckpointSigners { extra_data })
    }
    let mut signers = BTreeSet::new();
    for signer_bytes in extra_data[EXTRA_VANITY..extra_data.len() - EXTRA_SEAL].chunks(address_len)
    {
        signers.insert(Address::from_slice(signer_bytes));
    }
    Ok(signers)
}
