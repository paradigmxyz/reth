use crate::{
    AccessListPayload, BlobPayload, FeeMarketPayload, LegacyPayload, PayloadView, SetCodePayload,
    TransactionAuthorization, TransactionError, TransactionPayload, TransactionSsz,
};
use alloy_eips::{
    eip2930::{AccessList, AccessListItem},
    eip7702::{
        constants::{EIP7702_TX_TYPE_ID, MAGIC},
        Authorization,
    },
};
use alloy_primitives::{Address, Bytes, Signature, TxKind, B256, U256};
use ssz::{Decode, Encode, SszDecoderBuilder, SszEncoder};

pub fn encode_transaction(transaction: &TransactionSsz) -> Result<Vec<u8>, TransactionError> {
    let p = PayloadView::from(transaction.payload());
    let access = p.access_list.map(encode_access_list).transpose()?;
    let hashes = p.blob_hashes.map(encode_hashes).transpose()?;
    let authorizations = p
        .authorizations
        .map(|items| {
            encode_variable_list(
                items.iter().map(encode_authorization).collect::<Result<Vec<_>, _>>()?,
            )
        })
        .transpose()?;
    let fixed_length = payload_fixed_length(transaction.selector())?;
    let total = [Some(p.input), access.as_deref(), hashes.as_deref(), authorizations.as_deref()]
        .into_iter()
        .flatten()
        .try_fold(fixed_length, |length, bytes| {
            length.checked_add(bytes.len()).ok_or(TransactionError::LengthOverflow)
        })?;
    checked_offset(total)?;
    let mut payload = vec![transaction.selector()];
    let mut encoder = SszEncoder::container(&mut payload, fixed_length);
    encoder.append(&transaction.payload().transaction_type());
    if let Some(chain) = p.chain_id {
        encoder.append(&chain);
    }
    encoder.append(&p.nonce);
    encoder.append(&p.fee);
    if let Some(fee) = p.blob_fee {
        encoder.append(&fee);
    }
    encoder.append(&p.gas);
    if let Some(to) = p.to {
        encoder.append(&to);
    }
    encoder.append(&p.value);
    encoder.append_parameterized(false, |bytes| bytes.extend_from_slice(p.input));
    if let Some(access) = &access {
        encoder.append(access);
    }
    if let Some(priority) = p.priority_fee {
        encoder.append(&priority);
    }
    if let Some(hashes) = &hashes {
        encoder.append(hashes);
    }
    if let Some(authorizations) = &authorizations {
        encoder.append(authorizations);
    }
    encoder.finalize();
    if payload.len().checked_sub(1) != Some(total) {
        return Err(TransactionError::InvalidEncoding("payload layout length mismatch"));
    }
    encode_variable_list(vec![payload, signature_bytes(transaction.signature()).to_vec()])
}

pub fn decode_transaction(bytes: &[u8]) -> Result<TransactionSsz, TransactionError> {
    let fields = decode_variable_fields(bytes, 2)?;
    let union = fields[0];
    let selector =
        *union.first().ok_or(TransactionError::InvalidEncoding("missing payload selector"))?;
    let fixed_length = payload_fixed_length(selector)?;
    let data = &union[1..];
    if data.len() < fixed_length {
        return Err(TransactionError::InvalidEncoding("truncated payload"));
    }
    let expected_type = match selector {
        1..=4 => 0,
        5..=6 => 1,
        7..=8 => 2,
        9 => 3,
        10 => EIP7702_TX_TYPE_ID,
        _ => return Err(TransactionError::UnsupportedSelector(selector)),
    };
    let mut builder = SszDecoderBuilder::new(data);
    builder.register_type::<u8>()?;
    if selector >= 3 {
        builder.register_type::<U256>()?;
    }
    builder.register_type::<u64>()?;
    builder.register_type::<U256>()?;
    if selector == 9 {
        builder.register_type::<U256>()?;
    }
    builder.register_type::<u64>()?;
    let create = matches!(selector, 2 | 4 | 6 | 8);
    if !create {
        builder.register_type::<Address>()?;
    }
    builder.register_type::<U256>()?;
    builder.register_type::<Bytes>()?;
    if selector >= 5 {
        builder.register_type::<Vec<u8>>()?;
    }
    if selector >= 7 {
        builder.register_type::<U256>()?;
    }
    if selector >= 9 {
        builder.register_type::<Vec<u8>>()?;
    }
    let mut decoder = builder.build()?;
    if decoder.decode_next::<u8>()? != expected_type {
        return Err(TransactionError::InvalidEncoding("selector and transaction type disagree"));
    }
    let chain = if selector >= 3 { Some(decoder.decode_next()?) } else { None };
    let nonce = decoder.decode_next()?;
    let fee = decoder.decode_next()?;
    let blob_fee = if selector == 9 { Some(decoder.decode_next()?) } else { None };
    let gas_limit = decoder.decode_next()?;
    let to = if create { TxKind::Create } else { TxKind::Call(decoder.decode_next()?) };
    let value = decoder.decode_next()?;
    let input = decoder.decode_next()?;
    let access_list = if selector >= 5 {
        decode_access_list(decoder.decode_next_with(Ok)?)?
    } else {
        AccessList::default()
    };
    let priority = if selector >= 7 { Some(decoder.decode_next()?) } else { None };
    let extra = if selector >= 9 { decoder.decode_next_with(Ok)? } else { &[] };
    let payload = match selector {
        1..=4 => TransactionPayload::Legacy(LegacyPayload {
            chain_id: chain,
            nonce,
            gas_price: fee,
            gas_limit,
            to,
            value,
            input,
        }),
        5..=6 => TransactionPayload::AccessList(AccessListPayload {
            chain_id: chain.ok_or(TransactionError::InvalidEncoding("missing chain ID"))?,
            nonce,
            gas_price: fee,
            gas_limit,
            to,
            value,
            input,
            access_list,
        }),
        7..=8 => TransactionPayload::FeeMarket(FeeMarketPayload {
            chain_id: chain.ok_or(TransactionError::InvalidEncoding("missing chain ID"))?,
            nonce,
            max_fee_per_gas: fee,
            max_priority_fee_per_gas: priority
                .ok_or(TransactionError::InvalidEncoding("missing priority fee"))?,
            gas_limit,
            to,
            value,
            input,
            access_list,
        }),
        9 => TransactionPayload::Blob(BlobPayload {
            chain_id: chain.ok_or(TransactionError::InvalidEncoding("missing chain ID"))?,
            nonce,
            max_fee_per_gas: fee,
            max_priority_fee_per_gas: priority
                .ok_or(TransactionError::InvalidEncoding("missing priority fee"))?,
            max_fee_per_blob_gas: blob_fee
                .ok_or(TransactionError::InvalidEncoding("missing blob fee"))?,
            gas_limit,
            to: required_address(to)?,
            value,
            input,
            access_list,
            blob_versioned_hashes: decode_hashes(extra)?,
        }),
        10 => TransactionPayload::SetCode(SetCodePayload {
            chain_id: chain.ok_or(TransactionError::InvalidEncoding("missing chain ID"))?,
            nonce,
            max_fee_per_gas: fee,
            max_priority_fee_per_gas: priority
                .ok_or(TransactionError::InvalidEncoding("missing priority fee"))?,
            gas_limit,
            to: required_address(to)?,
            value,
            input,
            access_list,
            authorization_list: decode_variable_list(extra)?
                .into_iter()
                .map(decode_authorization)
                .collect::<Result<Vec<_>, _>>()?,
        }),
        _ => return Err(TransactionError::UnsupportedSelector(selector)),
    };
    let signature = fields[1];
    if signature.len() != 66 || signature[0] != 0 || signature[65] > 1 {
        return Err(TransactionError::InvalidSignature);
    }
    TransactionSsz::new(
        payload,
        Signature::new(
            U256::from_be_slice(&signature[1..33]),
            U256::from_be_slice(&signature[33..65]),
            signature[65] == 1,
        ),
    )
}

pub fn encode_transactions(transactions: &[TransactionSsz]) -> Result<Vec<u8>, TransactionError> {
    encode_variable_list(
        transactions.iter().map(encode_transaction).collect::<Result<Vec<_>, _>>()?,
    )
}

pub fn decode_transactions(bytes: &[u8]) -> Result<Vec<TransactionSsz>, TransactionError> {
    decode_variable_list(bytes)?.into_iter().map(decode_transaction).collect()
}

pub(crate) fn signature_bytes(signature: &Signature) -> [u8; 66] {
    let mut bytes = [0; 66];
    bytes[1..].copy_from_slice(&signature.as_rsy());
    bytes
}

pub(crate) fn authorization_selector(auth: &TransactionAuthorization) -> u8 {
    if auth.inner().chain_id.is_zero() {
        1
    } else {
        2
    }
}

pub(crate) fn authorization_signature_bytes(auth: &TransactionAuthorization) -> [u8; 66] {
    let mut bytes = [0; 66];
    bytes[1..33].copy_from_slice(&auth.r().to_be_bytes::<32>());
    bytes[33..65].copy_from_slice(&auth.s().to_be_bytes::<32>());
    bytes[65] = auth.y_parity();
    bytes
}

fn encode_access_list(access: &AccessList) -> Result<Vec<u8>, TransactionError> {
    encode_variable_list(
        access
            .0
            .iter()
            .map(|item| {
                let keys = encode_hashes(&item.storage_keys)?;
                checked_offset(
                    keys.len().checked_add(24).ok_or(TransactionError::LengthOverflow)?,
                )?;
                let mut bytes = Vec::new();
                let mut encoder = SszEncoder::container(&mut bytes, 24);
                encoder.append(&item.address);
                encoder.append(&keys);
                encoder.finalize();
                Ok(bytes)
            })
            .collect::<Result<Vec<_>, TransactionError>>()?,
    )
}

fn decode_access_list(bytes: &[u8]) -> Result<AccessList, TransactionError> {
    Ok(AccessList(
        decode_variable_list(bytes)?
            .into_iter()
            .map(|item| {
                let mut builder = SszDecoderBuilder::new(item);
                builder.register_type::<Address>()?;
                builder.register_type::<Vec<B256>>()?;
                let mut decoder = builder.build()?;
                Ok(AccessListItem {
                    address: decoder.decode_next()?,
                    storage_keys: decoder.decode_next()?,
                })
            })
            .collect::<Result<Vec<_>, TransactionError>>()?,
    ))
}

fn encode_hashes(hashes: &[B256]) -> Result<Vec<u8>, TransactionError> {
    let length = hashes.len().checked_mul(32).ok_or(TransactionError::LengthOverflow)?;
    checked_offset(length)?;
    let mut bytes = Vec::with_capacity(length);
    for hash in hashes {
        hash.ssz_append(&mut bytes);
    }
    Ok(bytes)
}

fn decode_hashes(bytes: &[u8]) -> Result<Vec<B256>, TransactionError> {
    if !bytes.len().is_multiple_of(32) {
        return Err(TransactionError::InvalidEncoding("incomplete hash-list element"));
    }
    Vec::<B256>::from_ssz_bytes(bytes).map_err(Into::into)
}

fn encode_authorization(auth: &TransactionAuthorization) -> Result<Vec<u8>, TransactionError> {
    let mut payload = vec![authorization_selector(auth), MAGIC];
    if !auth.inner().chain_id.is_zero() {
        auth.inner().chain_id.ssz_append(&mut payload);
    }
    auth.inner().address.ssz_append(&mut payload);
    auth.inner().nonce.ssz_append(&mut payload);
    encode_variable_list(vec![payload, authorization_signature_bytes(auth).to_vec()])
}

fn decode_authorization(bytes: &[u8]) -> Result<TransactionAuthorization, TransactionError> {
    let fields = decode_variable_fields(bytes, 2)?;
    let payload = fields[0];
    let (&selector, data) = payload
        .split_first()
        .ok_or(TransactionError::InvalidEncoding("missing authorization selector"))?;
    let mut builder = SszDecoderBuilder::new(data);
    builder.register_type::<u8>()?;
    match selector {
        1 => {}
        2 => builder.register_type::<U256>()?,
        _ => return Err(TransactionError::UnsupportedSelector(selector)),
    }
    builder.register_type::<Address>()?;
    builder.register_type::<u64>()?;
    let mut decoder = builder.build()?;
    if decoder.decode_next::<u8>()? != MAGIC {
        return Err(TransactionError::InvalidEncoding("invalid authorization magic"));
    }
    let chain = if selector == 2 { decoder.decode_next::<U256>()? } else { U256::ZERO };
    if selector == 2 && chain.is_zero() {
        return Err(TransactionError::InvalidEncoding(
            "explicit authorization chain must be nonzero",
        ));
    }
    let address = decoder.decode_next()?;
    let nonce = decoder.decode_next()?;
    let signature = fields[1];
    if signature.len() != 66 || signature[0] != 0 {
        return Err(TransactionError::InvalidEncoding("invalid authorization signature format"));
    }
    Ok(TransactionAuthorization::new_unchecked(
        Authorization { chain_id: chain, address, nonce },
        signature[65],
        U256::from_be_slice(&signature[1..33]),
        U256::from_be_slice(&signature[33..65]),
    ))
}

fn encode_variable_list(items: Vec<Vec<u8>>) -> Result<Vec<u8>, TransactionError> {
    let leading = items.len().checked_mul(4).ok_or(TransactionError::LengthOverflow)?;
    let total = items.iter().try_fold(leading, |length, bytes| {
        length.checked_add(bytes.len()).ok_or(TransactionError::LengthOverflow)
    })?;
    checked_offset(total)?;
    Ok(items.as_ssz_bytes())
}

fn decode_variable_list(bytes: &[u8]) -> Result<Vec<&[u8]>, TransactionError> {
    checked_offset(bytes.len())?;
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let leading = ssz::read_offset(bytes)?;
    if leading == 0 || !leading.is_multiple_of(4) || leading > bytes.len() {
        return Err(TransactionError::InvalidEncoding("invalid list offset table"));
    }
    let items = decode_variable_fields(bytes, leading / 4)?;
    if items.iter().any(|item| item.is_empty()) {
        return Err(TransactionError::InvalidEncoding("empty composite list element"));
    }
    Ok(items)
}

fn decode_variable_fields(bytes: &[u8], count: usize) -> Result<Vec<&[u8]>, TransactionError> {
    checked_offset(bytes.len())?;
    let mut builder = SszDecoderBuilder::new(bytes);
    for _ in 0..count {
        builder.register_type::<Vec<u8>>()?;
    }
    let mut decoder = builder.build()?;
    (0..count).map(|_| decoder.decode_next_with(Ok).map_err(Into::into)).collect()
}

const fn payload_fixed_length(selector: u8) -> Result<usize, TransactionError> {
    match selector {
        1 => Ok(105),
        2 => Ok(85),
        3 => Ok(137),
        4 => Ok(117),
        5 => Ok(141),
        6 => Ok(121),
        7 => Ok(173),
        8 => Ok(153),
        9 => Ok(209),
        10 => Ok(177),
        _ => Err(TransactionError::UnsupportedSelector(selector)),
    }
}

const fn required_address(to: TxKind) -> Result<Address, TransactionError> {
    if let TxKind::Call(address) = to {
        Ok(address)
    } else {
        Err(TransactionError::InvalidEncoding("transaction requires destination"))
    }
}

fn checked_offset(value: usize) -> Result<u32, TransactionError> {
    u32::try_from(value).map_err(|_| TransactionError::LengthOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_authorization_rejects_wrong_magic_algorithm_and_explicit_zero() {
        let auth = TransactionAuthorization::new_unchecked(
            Authorization { chain_id: U256::from(1), address: Address::ZERO, nonce: 0 },
            255,
            U256::ZERO,
            U256::MAX,
        );
        let bytes = encode_authorization(&auth).unwrap();
        assert_eq!(decode_authorization(&bytes).unwrap(), auth);
        let mut wrong_magic = bytes.clone();
        wrong_magic[9] = 4;
        assert!(decode_authorization(&wrong_magic).is_err());
        let mut explicit_zero = bytes.clone();
        explicit_zero[10..42].fill(0);
        assert!(matches!(
            decode_authorization(&explicit_zero),
            Err(TransactionError::InvalidEncoding("explicit authorization chain must be nonzero"))
        ));
        let mut algorithm = bytes.clone();
        let signature = ssz::read_offset(&algorithm[4..]).unwrap();
        algorithm[signature] = 1;
        assert!(decode_authorization(&algorithm).is_err());
        let mut wrong_offset = bytes;
        wrong_offset[..4].copy_from_slice(&9_u32.to_le_bytes());
        assert!(decode_authorization(&wrong_offset).is_err());
    }

    #[test]
    fn access_tuple_offsets_and_fixed_hash_widths_are_checked() {
        let access = AccessList(vec![AccessListItem {
            address: Address::ZERO,
            storage_keys: vec![B256::ZERO],
        }]);
        let bytes = encode_access_list(&access).unwrap();
        assert_eq!(decode_access_list(&bytes).unwrap(), access);
        let mut bad = bytes.clone();
        bad[24..28].copy_from_slice(&25_u32.to_le_bytes());
        assert!(decode_access_list(&bad).is_err());
        let mut incomplete = bytes;
        incomplete.pop();
        assert!(decode_access_list(&incomplete).is_err());
        assert!(decode_variable_list(&[4, 0, 0, 0]).is_err());
        assert!(decode_hashes(&[0; 31]).is_err());
        assert!(decode_hashes(&[0; 33]).is_err());
    }
}
