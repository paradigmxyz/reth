use super::{
    BasicReceipt, CreateReceipt, Log, Receipt, Receipts, SetCodeReceipt, BASIC_RECEIPT_SELECTOR,
    CREATE_RECEIPT_SELECTOR, MAX_TOPICS_PER_LOG, SET_CODE_RECEIPT_SELECTOR,
};
use alloy_primitives::{Address, Bytes, B256};
use ssz::{Decode, DecodeError, SszDecoderBuilder, MAX_LENGTH_VALUE};

fn check_decode_length(bytes: &[u8]) -> Result<(), DecodeError> {
    if bytes.len() > MAX_LENGTH_VALUE {
        return Err(DecodeError::BytesInvalid(format!(
            "SSZ input is {} bytes, maximum is {MAX_LENGTH_VALUE}",
            bytes.len()
        )));
    }

    Ok(())
}

impl Decode for Log {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        check_decode_length(bytes)?;

        let mut builder = SszDecoderBuilder::new(bytes);
        builder.register_type::<Address>()?;
        builder.register_type::<Vec<B256>>()?;
        builder.register_type::<Bytes>()?;

        let mut decoder = builder.build()?;
        let address = decoder.decode_next::<Address>()?;
        let topics = decoder.decode_next_with(|bytes| {
            if !bytes.len().is_multiple_of(32) {
                return Err(DecodeError::BytesInvalid(
                    "log topics must contain complete 32-byte values".to_owned(),
                ));
            }

            let count = bytes.len() / 32;
            if count > MAX_TOPICS_PER_LOG {
                return Err(DecodeError::BytesInvalid(format!(
                    "log has {count} topics, maximum is {MAX_TOPICS_PER_LOG}"
                )));
            }

            Vec::<B256>::from_ssz_bytes(bytes)
        })?;
        let data = decoder.decode_next::<Bytes>()?;

        Self::new(address, topics, data)
            .map_err(|error| DecodeError::BytesInvalid(error.to_string()))
    }
}

impl Decode for Receipt {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        check_decode_length(bytes)?;

        let (&selector, body) =
            bytes.split_first().ok_or(DecodeError::InvalidByteLength { len: 0, expected: 1 })?;

        if !matches!(
            selector,
            BASIC_RECEIPT_SELECTOR | CREATE_RECEIPT_SELECTOR | SET_CODE_RECEIPT_SELECTOR
        ) {
            return Err(DecodeError::UnionSelectorInvalid(selector));
        }

        let mut builder = SszDecoderBuilder::new(body);
        builder.register_type::<Address>()?;
        builder.register_type::<u64>()?;

        if selector == CREATE_RECEIPT_SELECTOR {
            builder.register_type::<Address>()?;
        }

        builder.register_type::<Vec<Log>>()?;
        builder.register_type::<bool>()?;

        if selector == SET_CODE_RECEIPT_SELECTOR {
            builder.register_type::<Vec<Address>>()?;
        }

        let mut decoder = builder.build()?;
        let from_ = decoder.decode_next::<Address>()?;
        let gas_used = decoder.decode_next::<u64>()?;

        match selector {
            BASIC_RECEIPT_SELECTOR => {
                let logs = decoder.decode_next::<Vec<Log>>()?;
                let status = decoder.decode_next::<bool>()?;
                Ok(Self::Basic(BasicReceipt { from_, gas_used, logs, status }))
            }
            CREATE_RECEIPT_SELECTOR => {
                let contract_address = decoder.decode_next::<Address>()?;
                let logs = decoder.decode_next::<Vec<Log>>()?;
                let status = decoder.decode_next::<bool>()?;
                Ok(Self::Create(CreateReceipt { from_, gas_used, contract_address, logs, status }))
            }
            SET_CODE_RECEIPT_SELECTOR => {
                let logs = decoder.decode_next::<Vec<Log>>()?;
                let status = decoder.decode_next::<bool>()?;
                let authorities = decoder.decode_next_with(|bytes| {
                    if !bytes.len().is_multiple_of(20) {
                        return Err(DecodeError::BytesInvalid(
                            "authorities must contain complete 20-byte values".to_owned(),
                        ));
                    }

                    Vec::<Address>::from_ssz_bytes(bytes)
                })?;
                Ok(Self::SetCode(SetCodeReceipt { from_, gas_used, logs, status, authorities }))
            }
            _ => Err(DecodeError::UnionSelectorInvalid(selector)),
        }
    }
}

impl Decode for Receipts {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        check_decode_length(bytes)?;
        Vec::<Receipt>::from_ssz_bytes(bytes).map(Self::new)
    }
}
