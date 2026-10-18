use alloy_eips::eip4895::Withdrawal;
use ssz::{Decode, DecodeError, Encode, MAX_LENGTH_VALUE};
use std::fmt;

pub const WITHDRAWAL_SSZ_LENGTH: usize = 44;

#[derive(Debug)]
pub enum WithdrawalsCodecError {
    LengthOverflow,
    TooLong { actual: usize, max: usize },
    Decode(DecodeError),
}

impl fmt::Display for WithdrawalsCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LengthOverflow => formatter.write_str("withdrawal serialization length overflow"),
            Self::TooLong { actual, max } => {
                write!(formatter, "withdrawal serialization is {actual} bytes, maximum is {max}")
            }
            Self::Decode(error) => {
                write!(formatter, "withdrawal decoding failed: {error:?}")
            }
        }
    }
}

impl std::error::Error for WithdrawalsCodecError {}

const fn check_length(length: usize) -> Result<(), WithdrawalsCodecError> {
    if length > MAX_LENGTH_VALUE {
        return Err(WithdrawalsCodecError::TooLong { actual: length, max: MAX_LENGTH_VALUE });
    }

    Ok(())
}

pub fn encode_withdrawal(withdrawal: &Withdrawal) -> Vec<u8> {
    withdrawal.as_ssz_bytes()
}

pub fn decode_withdrawal(bytes: &[u8]) -> Result<Withdrawal, WithdrawalsCodecError> {
    if bytes.len() != WITHDRAWAL_SSZ_LENGTH {
        return Err(WithdrawalsCodecError::Decode(DecodeError::InvalidByteLength {
            len: bytes.len(),
            expected: WITHDRAWAL_SSZ_LENGTH,
        }));
    }

    Withdrawal::from_ssz_bytes(bytes).map_err(WithdrawalsCodecError::Decode)
}

pub fn encode_withdrawals(withdrawals: &[Withdrawal]) -> Result<Vec<u8>, WithdrawalsCodecError> {
    let length = withdrawals
        .len()
        .checked_mul(WITHDRAWAL_SSZ_LENGTH)
        .ok_or(WithdrawalsCodecError::LengthOverflow)?;

    check_length(length)?;

    let mut bytes = Vec::with_capacity(length);
    for withdrawal in withdrawals {
        withdrawal.ssz_append(&mut bytes);
    }

    Ok(bytes)
}

pub fn decode_withdrawals(bytes: &[u8]) -> Result<Vec<Withdrawal>, WithdrawalsCodecError> {
    check_length(bytes.len())?;

    if !bytes.len().is_multiple_of(WITHDRAWAL_SSZ_LENGTH) {
        return Err(WithdrawalsCodecError::Decode(DecodeError::BytesInvalid(
            "withdrawal list must contain complete 44-byte elements".to_owned(),
        )));
    }

    Vec::<Withdrawal>::from_ssz_bytes(bytes).map_err(WithdrawalsCodecError::Decode)
}
