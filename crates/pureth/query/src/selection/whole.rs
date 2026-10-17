use super::{Kind, SelectionError};
use crate::proof::hash_pair;
use alloy_primitives::B256;
use reth_pureth_receipt::eip6466::{Log, Receipt, Receipts, MAX_TOPICS_PER_LOG};
use ssz::Decode;
use std::vec::IntoIter;

fn fixed(mut roots: Vec<B256>, width: usize) -> B256 {
    roots.resize(width, B256::ZERO);
    while roots.len() > 1 {
        let count = roots.len() / 2;
        for index in 0..count {
            roots[index] = hash_pair(&roots[index * 2], &roots[index * 2 + 1]);
        }
        roots.truncate(count);
    }
    roots[0]
}

fn progressive(roots: Vec<B256>) -> Result<B256, SelectionError> {
    progressive_level(&mut roots.into_iter(), 1)
}

fn progressive_level(roots: &mut IntoIter<B256>, width: usize) -> Result<B256, SelectionError> {
    if roots.len() == 0 {
        return Ok(B256::ZERO);
    }
    let left = fixed(roots.by_ref().take(width).collect(), width);
    let right = if roots.len() == 0 {
        B256::ZERO
    } else {
        progressive_level(
            roots,
            width.checked_mul(4).ok_or(SelectionError::LimitExceeded("proof depth"))?,
        )?
    };
    Ok(hash_pair(&left, &right))
}

fn mix_length(root: B256, length: usize) -> B256 {
    hash_pair(&root, &B256::right_padding_from(&length.to_le_bytes()))
}

fn bytes_root(bytes: &[u8]) -> Result<B256, SelectionError> {
    let roots = bytes.chunks(32).map(B256::right_padding_from).collect();
    Ok(mix_length(progressive(roots)?, bytes.len()))
}

fn log_root(log: &Log) -> Result<B256, SelectionError> {
    Ok(fixed(
        vec![
            B256::right_padding_from(log.address().as_slice()),
            mix_length(fixed(log.topics().to_vec(), MAX_TOPICS_PER_LOG), log.topics().len()),
            bytes_root(log.data())?,
        ],
        4,
    ))
}

fn logs_root(logs: &[Log]) -> Result<B256, SelectionError> {
    let roots = logs.iter().map(log_root).collect::<Result<_, _>>()?;
    Ok(mix_length(progressive(roots)?, logs.len()))
}

fn receipt_root(receipt: &Receipt) -> Result<B256, SelectionError> {
    let mut fields = vec![
        B256::right_padding_from(receipt.from_().as_slice()),
        B256::right_padding_from(&receipt.gas_used().to_le_bytes()),
        B256::ZERO,
        logs_root(receipt.logs())?,
        B256::right_padding_from(&[u8::from(receipt.status())]),
    ];
    match receipt {
        Receipt::Create(value) => {
            fields[2] = B256::right_padding_from(value.contract_address.as_slice());
        }
        Receipt::SetCode(value) => {
            let roots = value
                .authorities
                .iter()
                .map(|address| B256::right_padding_from(address.as_slice()))
                .collect();
            fields.push(mix_length(progressive(roots)?, value.authorities.len()));
        }
        Receipt::Basic(_) => {}
    }
    let mut active = B256::ZERO;
    for (index, enabled) in receipt.active_fields().iter().enumerate() {
        if *enabled {
            active[index / 8] |= 1 << (index % 8);
        }
    }
    let container = hash_pair(&progressive(fields)?, &active);
    Ok(hash_pair(&container, &B256::right_padding_from(&[receipt.selector()])))
}

pub(super) fn root(kind: Kind, value: &[u8]) -> Result<B256, SelectionError> {
    match kind {
        Kind::Receipts => {
            let receipts =
                Receipts::from_ssz_bytes(value).map_err(|_| SelectionError::InvalidValue)?;
            let roots = receipts.as_slice().iter().map(receipt_root).collect::<Result<_, _>>()?;
            Ok(mix_length(progressive(roots)?, receipts.len()))
        }
        Kind::Receipt(selector) => {
            let receipt =
                Receipt::from_ssz_bytes(value).map_err(|_| SelectionError::InvalidValue)?;
            if receipt.selector() != selector {
                return Err(SelectionError::InvalidValue);
            }
            receipt_root(&receipt)
        }
        Kind::Log => {
            log_root(&Log::from_ssz_bytes(value).map_err(|_| SelectionError::InvalidValue)?)
        }
        Kind::Logs => {
            logs_root(&Vec::<Log>::from_ssz_bytes(value).map_err(|_| SelectionError::InvalidValue)?)
        }
        Kind::Data => bytes_root(value),
        Kind::Topics | Kind::Authorities => {
            let width = if kind == Kind::Topics { 32 } else { 20 };
            if !value.len().is_multiple_of(width) ||
                (kind == Kind::Topics && value.len() / width > MAX_TOPICS_PER_LOG)
            {
                return Err(SelectionError::InvalidValue);
            }
            let roots = value.chunks_exact(width).map(B256::right_padding_from).collect();
            let contents = if kind == Kind::Topics {
                fixed(roots, MAX_TOPICS_PER_LOG)
            } else {
                progressive(roots)?
            };
            Ok(mix_length(contents, value.len() / width))
        }
        _ => Err(SelectionError::CodecRequired),
    }
}
