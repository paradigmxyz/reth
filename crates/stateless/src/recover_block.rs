use crate::validation::StatelessValidationError;
use alloc::vec::Vec;
use alloy_consensus::BlockHeader;
use alloy_primitives::Address;
use core::ops::Deref;
use reth_chainspec::EthereumHardforks;
use reth_ethereum_primitives::{Block, TransactionSigned};
use reth_primitives_traits::{Block as _, RecoveredBlock};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, Bytes};

/// Serialized uncompressed public key
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UncompressedPublicKey(#[serde_as(as = "Bytes")] pub [u8; 65]);

impl Deref for UncompressedPublicKey {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Verifies all transactions in a block against a list of public keys and signatures.
///
/// Returns a `RecoveredBlock`
pub(crate) fn recover_block_with_public_keys<ChainSpec>(
    block: Block,
    public_keys: Vec<UncompressedPublicKey>,
    chain_spec: &ChainSpec,
) -> Result<RecoveredBlock<Block>, StatelessValidationError>
where
    ChainSpec: EthereumHardforks,
{
    if block.body().transactions.len() != public_keys.len() {
        return Err(StatelessValidationError::Custom(
            "Number of public keys must match number of transactions",
        ));
    }

    // Determine if we're in the Homestead fork for signature validation
    let is_homestead = chain_spec.is_homestead_active_at_block(block.header().number());

    // Verify each transaction signature against its corresponding public key
    let senders = public_keys
        .iter()
        .zip(block.body().transactions())
        .map(|(vk, tx)| verify_and_compute_sender(vk, tx, is_homestead))
        .collect::<Result<Vec<_>, _>>()?;

    // Create RecoveredBlock with verified senders
    let block_hash = block.hash_slow();
    Ok(RecoveredBlock::new(block, senders, block_hash))
}

/// Verifies a transaction using its signature and the given public key.
///
/// Note: If the signature or the public key is incorrect, then this method
/// will return an error.
///
/// Returns the address derived from the public key.
fn verify_and_compute_sender(
    vk: &UncompressedPublicKey,
    tx: &TransactionSigned,
    is_homestead: bool,
) -> Result<Address, StatelessValidationError> {
    let sig = tx.signature();

    // non-normalized signatures are only valid pre-homestead
    let sig_is_normalized = sig.normalize_s().is_none();
    if is_homestead && !sig_is_normalized {
        return Err(StatelessValidationError::HomesteadSignatureNotNormalized);
    }
    let sig_hash = tx.signature_hash();
    alloy_consensus::crypto::secp256k1::verify_and_compute_signer_unchecked(&vk.0, sig, sig_hash)
        .map_err(|_| StatelessValidationError::SignerRecovery)
}
