use crate::{
    AccessListPayload, BlobPayload, FeeMarketPayload, LegacyPayload, SetCodePayload,
    TransactionError, TransactionPayload, TransactionSsz,
};
use alloy_eips::{eip2930::AccessList, eip7702::SignedAuthorization};
use alloy_primitives::{Address, Bytes, Signature, TxKind, B256, U256, U512};
use alloy_rlp::{Decodable, Encodable, Header};

fn append<T: Encodable + ?Sized>(bytes: &mut Vec<u8>, value: &T) {
    value.encode(bytes);
}

fn list(fields: Vec<u8>) -> Vec<u8> {
    let mut bytes = Vec::new();
    Header { list: true, payload_length: fields.len() }.encode(&mut bytes);
    bytes.extend(fields);
    bytes
}

pub(crate) fn encode(transaction: &TransactionSsz, signed: bool) -> Vec<u8> {
    let mut fields = Vec::new();
    match transaction.payload() {
        TransactionPayload::Legacy(p) => {
            append(&mut fields, &p.nonce);
            append(&mut fields, &p.gas_price);
            append(&mut fields, &p.gas_limit);
            append(&mut fields, &p.to);
            append(&mut fields, &p.value);
            append(&mut fields, &p.input);
            if signed {
                let v = p.chain_id.map_or_else(
                    || U512::from(27 + u8::from(transaction.signature().v())),
                    |chain| {
                        U512::from(chain) * U512::from(2) +
                            U512::from(35 + u8::from(transaction.signature().v()))
                    },
                );
                append(&mut fields, &v);
            } else if let Some(chain) = p.chain_id {
                append(&mut fields, &chain);
                append(&mut fields, &0_u8);
                append(&mut fields, &0_u8);
            }
        }
        TransactionPayload::AccessList(p) => {
            append(&mut fields, &p.chain_id);
            append(&mut fields, &p.nonce);
            append(&mut fields, &p.gas_price);
            append(&mut fields, &p.gas_limit);
            append(&mut fields, &p.to);
            append(&mut fields, &p.value);
            append(&mut fields, &p.input);
            append(&mut fields, &p.access_list);
        }
        TransactionPayload::FeeMarket(p) => {
            append(&mut fields, &p.chain_id);
            append(&mut fields, &p.nonce);
            append(&mut fields, &p.max_priority_fee_per_gas);
            append(&mut fields, &p.max_fee_per_gas);
            append(&mut fields, &p.gas_limit);
            append(&mut fields, &p.to);
            append(&mut fields, &p.value);
            append(&mut fields, &p.input);
            append(&mut fields, &p.access_list);
        }
        TransactionPayload::Blob(p) => {
            append(&mut fields, &p.chain_id);
            append(&mut fields, &p.nonce);
            append(&mut fields, &p.max_priority_fee_per_gas);
            append(&mut fields, &p.max_fee_per_gas);
            append(&mut fields, &p.gas_limit);
            append(&mut fields, &p.to);
            append(&mut fields, &p.value);
            append(&mut fields, &p.input);
            append(&mut fields, &p.access_list);
            append(&mut fields, &p.max_fee_per_blob_gas);
            append(&mut fields, &p.blob_versioned_hashes);
        }
        TransactionPayload::SetCode(p) => {
            append(&mut fields, &p.chain_id);
            append(&mut fields, &p.nonce);
            append(&mut fields, &p.max_priority_fee_per_gas);
            append(&mut fields, &p.max_fee_per_gas);
            append(&mut fields, &p.gas_limit);
            append(&mut fields, &p.to);
            append(&mut fields, &p.value);
            append(&mut fields, &p.input);
            append(&mut fields, &p.access_list);
            append(&mut fields, &p.authorization_list);
        }
    }
    if signed {
        if transaction.payload().transaction_type() != 0 {
            append(&mut fields, &u8::from(transaction.signature().v()));
        }
        append(&mut fields, &transaction.signature().r());
        append(&mut fields, &transaction.signature().s());
    }
    let mut bytes = Vec::new();
    let type_ = transaction.payload().transaction_type();
    if type_ != 0 {
        bytes.push(type_);
    }
    bytes.extend(list(fields));
    bytes
}

fn read<T: Decodable>(bytes: &mut &[u8]) -> Result<T, TransactionError> {
    T::decode(bytes).map_err(TransactionError::Rlp)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<TransactionSsz, TransactionError> {
    let first = *bytes.first().ok_or(TransactionError::InvalidEncoding("empty transaction RLP"))?;
    let (type_, mut input) = if first >= 0xc0 { (0, bytes) } else { (first, &bytes[1..]) };
    if type_ > 4 || (type_ == 0 && first < 0xc0) {
        return Err(TransactionError::UnsupportedTransactionType(type_));
    }
    let mut fields = Header::decode_bytes(&mut input, true).map_err(TransactionError::Rlp)?;
    if !input.is_empty() {
        return Err(TransactionError::TrailingBytes);
    }
    let payload;
    let parity;
    if type_ == 0 {
        let nonce = read(&mut fields)?;
        let gas_price = read(&mut fields)?;
        let gas_limit = read(&mut fields)?;
        let to = read(&mut fields)?;
        let value = read(&mut fields)?;
        let input = read(&mut fields)?;
        let v: U512 = read(&mut fields)?;
        let chain_id;
        if v == U512::from(27) || v == U512::from(28) {
            parity = v == U512::from(28);
            chain_id = None;
        } else if v >= U512::from(35) {
            let adjusted = v - U512::from(35);
            parity = adjusted.bit(0);
            let chain = adjusted >> 1_usize;
            if chain > U512::from(U256::MAX) {
                return Err(TransactionError::InvalidEncoding("legacy chain ID exceeds uint256"));
            }
            let limbs = chain.as_limbs();
            chain_id = Some(U256::from_limbs([limbs[0], limbs[1], limbs[2], limbs[3]]));
        } else {
            return Err(TransactionError::InvalidSignature);
        }
        payload = TransactionPayload::Legacy(LegacyPayload {
            chain_id,
            nonce,
            gas_price,
            gas_limit,
            to,
            value,
            input,
        });
    } else {
        let chain_id = read(&mut fields)?;
        let nonce = read(&mut fields)?;
        let priority = if type_ >= 2 { read(&mut fields)? } else { U256::ZERO };
        let fee = read(&mut fields)?;
        let gas_limit = read(&mut fields)?;
        let to: TxKind = read(&mut fields)?;
        let value = read(&mut fields)?;
        let input: Bytes = read(&mut fields)?;
        let access_list: AccessList = read(&mut fields)?;
        payload = match type_ {
            1 => TransactionPayload::AccessList(AccessListPayload {
                chain_id,
                nonce,
                gas_price: fee,
                gas_limit,
                to,
                value,
                input,
                access_list,
            }),
            2 => TransactionPayload::FeeMarket(FeeMarketPayload {
                chain_id,
                nonce,
                max_fee_per_gas: fee,
                max_priority_fee_per_gas: priority,
                gas_limit,
                to,
                value,
                input,
                access_list,
            }),
            3 => TransactionPayload::Blob(BlobPayload {
                chain_id,
                nonce,
                max_fee_per_gas: fee,
                max_priority_fee_per_gas: priority,
                max_fee_per_blob_gas: read(&mut fields)?,
                gas_limit,
                to: address(to)?,
                value,
                input,
                access_list,
                blob_versioned_hashes: read::<Vec<B256>>(&mut fields)?,
            }),
            4 => TransactionPayload::SetCode(SetCodePayload {
                chain_id,
                nonce,
                max_fee_per_gas: fee,
                max_priority_fee_per_gas: priority,
                gas_limit,
                to: address(to)?,
                value,
                input,
                access_list,
                authorization_list: read::<Vec<SignedAuthorization>>(&mut fields)?,
            }),
            _ => return Err(TransactionError::UnsupportedTransactionType(type_)),
        };
        let raw: u8 = read(&mut fields)?;
        if raw > 1 {
            return Err(TransactionError::InvalidSignature);
        }
        parity = raw == 1;
    }
    let r = read(&mut fields)?;
    let s = read(&mut fields)?;
    if !fields.is_empty() {
        return Err(TransactionError::TrailingBytes);
    }
    TransactionSsz::new(payload, Signature::new(r, s, parity))
}

const fn address(to: TxKind) -> Result<Address, TransactionError> {
    match to {
        TxKind::Call(address) => Ok(address),
        TxKind::Create => {
            Err(TransactionError::InvalidEncoding("transaction requires destination"))
        }
    }
}
