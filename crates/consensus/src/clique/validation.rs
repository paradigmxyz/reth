use super::{config::CliqueConfig, constants::*};
use reth_interfaces::consensus::{CliqueError, Error};
use reth_primitives::{Address, ChainSpec, SealedHeader, EMPTY_OMMER_ROOT, H64, U256};
use std::time::SystemTime;

/// Validate
pub fn validate_header_standalone(
    chain_spec: ChainSpec,
    config: CliqueConfig, // TODO: should be part of chainspec
    header: &SealedHeader,
) -> Result<(), Error> {
    // Gas used needs to be less then gas limit. Gas used is going to be check after execution.
    if header.gas_used > header.gas_limit {
        return Err(Error::HeaderGasUsedExceedsGasLimit {
            gas_used: header.gas_used,
            gas_limit: header.gas_limit,
        })
    }

    // Check if timestamp is in future.
    let present_timestamp =
        SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
    if header.timestamp > present_timestamp {
        return Err(Error::TimestampIsInFuture { timestamp: header.timestamp, present_timestamp })
    }

    let is_checkpoint = header.number % config.epoch == 0;

    // Checkpoint blocks need to enforce zero beneficiary
    if is_checkpoint && !header.beneficiary.is_zero() {
        return Err(CliqueError::CheckpointBeneficiary { beneficiary: header.beneficiary }.into())
    }

    // Nonces must be 0x00..0 or 0xff..f, zeroes enforced on checkpoints
    let nonce = H64::from_low_u64_ne(header.nonce);
    if is_checkpoint && nonce.as_bytes() != NONCE_DROP_VOTE {
        return Err(CliqueError::CheckpointVote { nonce: header.nonce }.into())
    } else if nonce.as_bytes() != NONCE_AUTH_VOTE {
        return Err(CliqueError::InvalidVote { nonce: header.nonce }.into())
    }

    // Check that the extra-data contains both the vanity and signature
    let extra_data_len = header.extra_data.len();
    if extra_data_len < EXTRA_VANITY {
        return Err(CliqueError::MissingVanity { extra_data: header.extra_data.clone() }.into())
    } else if extra_data_len < EXTRA_VANITY + EXTRA_SEAL {
        return Err(CliqueError::MissingSignature { extra_data: header.extra_data.clone() }.into())
    }

    // Ensure that the extra-data contains a signer list on checkpoint, but none otherwise
    // Safe subtraction because of the check above.
    let signers_bytes_len = extra_data_len - EXTRA_VANITY - EXTRA_SEAL;
    if is_checkpoint && signers_bytes_len != 0 {
        return Err(CliqueError::ExtraSignerList { extra_data: header.extra_data.clone() }.into())
    } else if signers_bytes_len % Address::len_bytes() != 0 {
        return Err(CliqueError::CheckpointSigners { extra_data: header.extra_data.clone() }.into())
    }

    // Ensure that the mix digest is zero as we don't have fork protection currently
    if !header.mix_hash.is_zero() {
        return Err(CliqueError::NonZeroMixHash { mix_hash: header.mix_hash }.into())
    }

    // Ensure that the block doesn't contain any uncles which are meaningless in PoA
    if header.ommers_hash != EMPTY_OMMER_ROOT {
        return Err(Error::BodyOmmersHashDiff {
            got: header.ommers_hash,
            expected: EMPTY_OMMER_ROOT,
        })
    }

    // Ensure that the block's difficulty is meaningful (may not be correct at this point)
    if header.number > 0 {
        if header.difficulty != U256::from(DIFF_INTURN) ||
            header.difficulty != U256::from(DIFF_NOTURN)
        {
            return Err(CliqueError::Difficulty { difficulty: header.difficulty }.into())
        }
    }

    // // Verify that the gas limit is <= 2^63-1
    // if header.GasLimit > params.MaxGasLimit {
    //     return fmt.Errorf("invalid gasLimit: have %v, max %v", header.GasLimit,
    // params.MaxGasLimit) }
    // TODO:

    // if chain.Config().IsShanghai(header.Time) {
    //     return fmt.Errorf("clique does not support shanghai fork")
    // }
    // TODO:

    // // If all checks passed, validate any special fields for hard forks
    // if err := misc.VerifyForkHashes(chain.Config(), header, false); err != nil {
    //     return err
    // }
    // TODO:

    // // All basic checks passed, verify cascading fields
    // return c.verifyCascadingFields(chain, header, parents)
    todo!()
}

/// TODO:
pub fn validate_header_regarding_parent(
    parent: &SealedHeader,
    child: &SealedHeader,
    config: CliqueConfig, // TODO: should be part of chainspec
) -> Result<(), Error> {
    // // Ensure that the block's timestamp isn't too close to its parent
    // var parent *types.Header
    // if len(parents) > 0 {
    // 	parent = parents[len(parents)-1]
    // } else {
    // 	parent = chain.GetHeader(header.ParentHash, number-1)
    // }

    // if parent == nil || parent.Number.Uint64() != number-1 || parent.Hash() != header.ParentHash
    // { 	return consensus.ErrUnknownAncestor
    // }

    // if parent.Time+c.config.Period > header.Time {
    // 	return errInvalidTimestamp
    // }
    let expected_at_least = parent.timestamp + config.period;
    if child.timestamp < expected_at_least {
        return Err(CliqueError::Timestamp { expected_at_least, received: child.timestamp }.into())
    }

    // if !chain.Config().IsLondon(header.Number) {
    // 	// Verify BaseFee not present before EIP-1559 fork.
    // 	if header.BaseFee != nil {
    // 		return fmt.Errorf("invalid baseFee before fork: have %d, want <nil>", header.BaseFee)
    // 	}
    // 	if err := misc.VerifyGaslimit(parent.GasLimit, header.GasLimit); err != nil {
    // 		return err
    // 	}
    // } else if err := misc.VerifyEip1559Header(chain.Config(), parent, header); err != nil {
    // 	// Verify the header's EIP-1559 attributes.
    // 	return err
    // }
    // // Retrieve the snapshot needed to verify this header and cache it
    // snap, err := c.snapshot(chain, number-1, header.ParentHash, parents)
    // if err != nil {
    // 	return err
    // }
    // // If the block is a checkpoint block, verify the signer list
    // if number%c.config.Epoch == 0 {
    // 	signers := make([]byte, len(snap.Signers)*common.AddressLength)
    // 	for i, signer := range snap.signers() {
    // 		copy(signers[i*common.AddressLength:], signer[:])
    // 	}
    // 	extraSuffix := len(header.Extra) - extraSeal
    // 	if !bytes.Equal(header.Extra[extraVanity:extraSuffix], signers) {
    // 		return errMismatchingCheckpointSigners
    // 	}
    // }
    // // All basic checks passed, verify the seal and return
    // return c.verifySeal(snap, header, parents)

    Ok(())
}
