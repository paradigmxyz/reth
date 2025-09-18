use crate::validation::StatelessValidationError;
use alloc::vec::Vec;
use alloy_consensus::BlockHeader;
use alloy_primitives::Address;
use reth_chainspec::EthereumHardforks;
use reth_ethereum_primitives::{Block, TransactionSigned};
use reth_primitives_traits::{Block as _, RecoveredBlock};

#[cfg(all(feature = "k256", feature = "secp256k1"))]
compile_error!("Features 'k256' and 'secp256k1' are mutually exclusive");

#[cfg(not(any(feature = "k256", feature = "secp256k1")))]
compile_error!("Either 'k256' or 'secp256k1' feature must be enabled");

/// Serialized uncompressed public key
pub type UncompressedPublicKey = [u8; 65];

/// Verifies a transaction using its signature and the given public key.
///
/// Note: If the signature or the public key is incorrect, then this method
/// will return an error.
///
/// Returns the address derived from the public key.
#[cfg(feature = "k256")]
fn recover_sender(
    vk: &UncompressedPublicKey,
    tx: &TransactionSigned,
    is_homestead: bool,
) -> Result<Address, StatelessValidationError> {
    use k256::ecdsa::{signature::hazmat::PrehashVerifier, VerifyingKey};

    let sig = tx.signature();
    let vk =
        VerifyingKey::from_sec1_bytes(vk).map_err(|_| StatelessValidationError::SignerRecovery)?;

    // non-normalized signatures are only valid pre-homestead
    let sig_is_normalized = sig.normalize_s().is_none();
    if is_homestead && !sig_is_normalized {
        return Err(StatelessValidationError::HomesteadSignatureNotNormalized);
    }

    sig.to_k256()
        .and_then(|sig| vk.verify_prehash(tx.signature_hash().as_slice(), &sig))
        .map_err(|_| StatelessValidationError::SignerRecovery)?;

    Ok(Address::from_public_key(&vk))
}

/// Verifies a transaction using its signature and the given public key.
///
/// Note: If the signature or the public key is incorrect, then this method
/// will return an error.
///
/// Returns the address derived from the public key.
#[cfg(feature = "secp256k1")]
fn recover_sender(
    vk: &UncompressedPublicKey,
    tx: &TransactionSigned,
    is_homestead: bool,
) -> Result<Address, StatelessValidationError> {
    use secp256k1::{ecdsa::Signature, Message, PublicKey, SECP256K1};

    let sig = tx.signature();

    let public_key =
        PublicKey::from_slice(vk).map_err(|_| StatelessValidationError::SignerRecovery)?;

    let sig_is_normalized = sig.normalize_s().is_none();
    if is_homestead && !sig_is_normalized {
        return Err(StatelessValidationError::HomesteadSignatureNotNormalized);
    }

    let mut sig_bytes = [0u8; 64];
    sig_bytes[0..32].copy_from_slice(&sig.r().to_be_bytes::<32>());
    sig_bytes[32..64].copy_from_slice(&sig.s().to_be_bytes::<32>());

    let signature = Signature::from_compact(&sig_bytes)
        .map_err(|_| StatelessValidationError::SignerRecovery)?;

    let message = Message::from_digest(tx.signature_hash().0);
    SECP256K1
        .verify_ecdsa(&message, &signature, &public_key)
        .map_err(|_| StatelessValidationError::SignerRecovery)?;

    Ok(Address::from_raw_public_key(&vk[1..]))
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
        .map(|(vk, tx)| recover_sender(vk, tx, is_homestead))
        .collect::<Result<Vec<_>, _>>()?;

    // Create RecoveredBlock with verified senders
    let block_hash = block.hash_slow();
    Ok(RecoveredBlock::new(block, senders, block_hash))
}
