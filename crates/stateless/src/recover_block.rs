use crate::validation::StatelessValidationError;
use alloc::vec::Vec;
use alloy_consensus::BlockHeader;
use alloy_primitives::{Address, Signature, B256};
use reth_chainspec::EthereumHardforks;
use reth_ethereum_primitives::{Block, TransactionSigned};
use reth_primitives_traits::{Block as _, RecoveredBlock};

#[cfg(all(feature = "k256", feature = "secp256k1"))]
use k256 as _;

/// Serialized uncompressed public key
pub type UncompressedPublicKey = [u8; 65];

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
    #[cfg(all(feature = "k256", feature = "secp256k1"))]
    {
        let _ = verify_and_compute_sender_unchecked_k256;
    }
    #[cfg(feature = "secp256k1")]
    {
        verify_and_compute_sender_unchecked_secp256k1(vk, sig, sig_hash)
    }
    #[cfg(all(feature = "k256", not(feature = "secp256k1")))]
    {
        verify_and_compute_sender_unchecked_k256(vk, sig, sig_hash)
    }
    #[cfg(not(any(feature = "secp256k1", feature = "k256")))]
    {
        let _ = vk;
        let _ = tx;
        let _: B256 = sig_hash;
        let _: &Signature = sig;

        unimplemented!("Must choose either k256 or secp256k1 feature")
    }
}
#[cfg(feature = "k256")]
fn verify_and_compute_sender_unchecked_k256(
    vk: &UncompressedPublicKey,
    sig: &Signature,
    sig_hash: B256,
) -> Result<Address, StatelessValidationError> {
    use k256::ecdsa::{signature::hazmat::PrehashVerifier, VerifyingKey};

    let vk =
        VerifyingKey::from_sec1_bytes(vk).map_err(|_| StatelessValidationError::SignerRecovery)?;

    sig.to_k256()
        .and_then(|sig| vk.verify_prehash(sig_hash.as_slice(), &sig))
        .map_err(|_| StatelessValidationError::SignerRecovery)?;

    Ok(Address::from_public_key(&vk))
}

#[cfg(feature = "secp256k1")]
fn verify_and_compute_sender_unchecked_secp256k1(
    vk: &UncompressedPublicKey,
    sig: &Signature,
    sig_hash: B256,
) -> Result<Address, StatelessValidationError> {
    use secp256k1::{ecdsa::Signature as SecpSignature, Message, PublicKey, SECP256K1};

    let public_key =
        PublicKey::from_slice(vk).map_err(|_| StatelessValidationError::SignerRecovery)?;

    let mut sig_bytes = [0u8; 64];
    sig_bytes[0..32].copy_from_slice(&sig.r().to_be_bytes::<32>());
    sig_bytes[32..64].copy_from_slice(&sig.s().to_be_bytes::<32>());

    let signature = SecpSignature::from_compact(&sig_bytes)
        .map_err(|_| StatelessValidationError::SignerRecovery)?;

    let message = Message::from_digest(sig_hash.0);
    SECP256K1
        .verify_ecdsa(&message, &signature, &public_key)
        .map_err(|_| StatelessValidationError::SignerRecovery)?;

    Ok(Address::from_raw_public_key(&vk[1..]))
}
