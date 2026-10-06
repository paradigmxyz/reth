use crate::{
    compose_gindices, container_field_gindex, parse_path, progressive_chunk_gindex,
    proof_access::node_and_branch, verify_branch, PathToken,
};
use alloy_primitives::{Address, Bytes, B256};
use reth_pureth_receipt::{
    eip6466::{BasicReceipt, Eip6466ReceiptSnapshot, Log, Receipt, Receipts},
    RetainedNode,
};
use reth_pureth_ssz::{
    merkleize_fixed, merkleize_progressive, mix_in_length, progressive_byte_list,
};
use serde::{Deserialize, Serialize};
use ssz::{read_offset, Decode, SszDecoderBuilder};
use std::{
    cell::Cell,
    collections::BTreeMap,
    io::{self, Write},
    mem::{size_of, size_of_val},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionRequest {
    pub selections: Vec<ReceiptSelection>,
    pub include_proof: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptSelection {
    pub path: String,
    pub operation: SelectionOperation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectionOperation {
    Value {},
    Whole {},
    Length {},
    Variant {},
    Presence {},
    Range { start: u64, end: u64 },
    Slice { start: u64, end: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionResponse {
    pub root: B256,
    pub proof_format: Option<SelectionProofFormat>,
    pub results: Vec<SelectionResult>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionProofFormat {
    SingleBranch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionResult {
    pub values_ssz: Vec<Bytes>,
    pub witnesses: Option<Vec<SelectionWitness>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionWitness {
    pub node: B256,
    pub branch: Vec<B256>,
}

#[derive(Clone, Copy, Debug)]
pub struct SelectionLimits {
    pub request_bytes: usize,
    pub selections: usize,
    pub targets: usize,
    pub witness_hashes: usize,
    pub response_bytes: usize,
    pub source_bytes: usize,
    pub receipts: usize,
    pub logs: usize,
}

impl Default for SelectionLimits {
    fn default() -> Self {
        Self {
            request_bytes: 64 * 1024,
            selections: 64,
            targets: 1024,
            witness_hashes: 16_384,
            response_bytes: 8 * 1024 * 1024,
            source_bytes: 64 * 1024 * 1024,
            receipts: 4096,
            logs: 16_384,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionError {
    InvalidPath,
    UnsupportedOperation,
    AbsentField,
    OutOfBounds,
    InvalidContext,
    InvalidValue,
    InvalidProof,
    CodecRequired,
    DuplicateOrOverlap,
    Unverified,
    Cancelled,
    Deadline,
    ExecutionFailed,
    LimitExceeded(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Scalar(usize),
    Byte(usize),
    Receipt(u8),
    Log,
    Receipts,
    Logs,
    Topics,
    Data,
    Authorities,
    Absent,
}

#[derive(Clone, Copy)]
struct Target {
    index: u128,
    kind: Kind,
    length: Option<u64>,
}

fn join(parent: u128, child: u128) -> Result<u128, SelectionError> {
    compose_gindices(parent, child).map_err(|_| SelectionError::LimitExceeded("proof depth"))
}

fn progressive(parent: u128, index: u64) -> Result<u128, SelectionError> {
    join(parent, progressive_chunk_gindex(index).map_err(|_| SelectionError::InvalidContext)?)
}

fn small(node: B256, width: usize) -> Result<Vec<u8>, SelectionError> {
    if node[width..].iter().any(|byte| *byte != 0) {
        return Err(SelectionError::InvalidValue);
    }
    Ok(node[..width].to_vec())
}

fn length(node: B256) -> Result<u64, SelectionError> {
    let bytes = small(node, 8)?;
    Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| SelectionError::InvalidContext)?))
}

fn collection(
    index: u128,
    kind: Kind,
    read: &mut impl FnMut(u128) -> Result<B256, SelectionError>,
) -> Result<Target, SelectionError> {
    Ok(Target { index, kind, length: Some(length(read(join(index, 3)?)?)?) })
}

const fn scalar(index: u128, width: usize) -> Target {
    Target { index, kind: Kind::Scalar(width), length: None }
}

fn resolve_eip(
    tokens: &[PathToken],
    read: &mut impl FnMut(u128) -> Result<B256, SelectionError>,
) -> Result<Target, SelectionError> {
    let root = collection(1, Kind::Receipts, read)?;
    if tokens.is_empty() {
        return Ok(root);
    }
    let [PathToken::Index(receipt_index), rest @ ..] = tokens else {
        return Err(SelectionError::InvalidPath);
    };
    if *receipt_index >= root.length.unwrap_or(0) {
        return Err(SelectionError::OutOfBounds);
    }
    let receipt = progressive(1, *receipt_index)?;
    let selector = small(read(join(receipt, 3)?)?, 1)?[0];
    let mask = match selector {
        1 => 0x1b,
        2 => 0x1f,
        3 => 0x3b,
        _ => return Err(SelectionError::InvalidContext),
    };
    let container = join(receipt, 2)?;
    if small(read(join(container, 3)?)?, 1)?[0] != mask {
        return Err(SelectionError::InvalidContext);
    }
    if rest.is_empty() {
        return Ok(Target { index: receipt, kind: Kind::Receipt(selector), length: None });
    }
    let [PathToken::Field(field), rest @ ..] = rest else {
        return Err(SelectionError::InvalidPath);
    };
    let (field_index, kind) = match field.as_str() {
        "from" => (0, Kind::Scalar(20)),
        "gas_used" => (1, Kind::Scalar(8)),
        "contract_address" => (2, if selector == 2 { Kind::Scalar(20) } else { Kind::Absent }),
        "logs" => (3, Kind::Logs),
        "status" => (4, Kind::Scalar(1)),
        "authorities" => (5, if selector == 3 { Kind::Authorities } else { Kind::Absent }),
        _ => return Err(SelectionError::InvalidPath),
    };
    if kind == Kind::Absent && rest.is_empty() {
        return Ok(Target { index: join(container, 3)?, kind, length: None });
    }
    if kind == Kind::Absent {
        return Err(SelectionError::AbsentField);
    }
    let index = progressive(container, field_index)?;
    if let Kind::Scalar(width) = kind {
        if !rest.is_empty() {
            return Err(SelectionError::InvalidPath);
        }
        return Ok(scalar(index, width));
    }
    let list = collection(index, kind, read)?;
    if rest.is_empty() {
        return Ok(list);
    }
    let [PathToken::Index(element), rest @ ..] = rest else {
        return Err(SelectionError::InvalidPath);
    };
    if *element >= list.length.unwrap_or(0) {
        return Err(SelectionError::OutOfBounds);
    }
    if kind == Kind::Authorities && rest.is_empty() {
        return Ok(scalar(progressive(index, *element)?, 20));
    }
    if kind != Kind::Logs {
        return Err(SelectionError::InvalidPath);
    }
    let log = progressive(index, *element)?;
    if rest.is_empty() {
        return Ok(Target { index: log, kind: Kind::Log, length: None });
    }
    let [PathToken::Field(field), rest @ ..] = rest else {
        return Err(SelectionError::InvalidPath);
    };
    let (field_index, kind) = match field.as_str() {
        "address" => (0, Kind::Scalar(20)),
        "topics" => (1, Kind::Topics),
        "data" => (2, Kind::Data),
        _ => return Err(SelectionError::InvalidPath),
    };
    let field = join(
        log,
        container_field_gindex(3, field_index).map_err(|_| SelectionError::InvalidPath)?,
    )?;
    if let Kind::Scalar(width) = kind {
        if !rest.is_empty() {
            return Err(SelectionError::InvalidPath);
        }
        return Ok(scalar(field, width));
    }
    let target = collection(field, kind, read)?;
    if kind == Kind::Topics && target.length.unwrap_or(0) > 4 {
        return Err(SelectionError::InvalidContext);
    }
    if rest.is_empty() {
        return Ok(target);
    }
    let [PathToken::Index(element)] = rest else {
        return Err(SelectionError::InvalidPath);
    };
    if *element >= target.length.unwrap_or(0) {
        return Err(SelectionError::OutOfBounds);
    }
    if kind == Kind::Topics {
        return Ok(scalar(join(field, 8 + u128::from(*element))?, 32));
    }
    check_padding(target, read)?;
    Ok(Target {
        index: progressive(field, *element / 32)?,
        kind: Kind::Byte((*element % 32) as usize),
        length: None,
    })
}

fn check_padding(
    target: Target,
    read: &mut impl FnMut(u128) -> Result<B256, SelectionError>,
) -> Result<(), SelectionError> {
    let length = target.length.ok_or(SelectionError::UnsupportedOperation)?;
    if length % 32 != 0 {
        let node = read(progressive(target.index, length / 32)?)?;
        if node[(length % 32) as usize..].iter().any(|byte| *byte != 0) {
            return Err(SelectionError::InvalidValue);
        }
    }
    Ok(())
}

fn tokens(path: &str) -> Result<Vec<PathToken>, SelectionError> {
    if path == "." {
        Ok(Vec::new())
    } else {
        parse_path(path).map_err(|_| SelectionError::InvalidPath)
    }
}

fn claim_value(
    target: Target,
    read: &mut impl FnMut(u128) -> Result<B256, SelectionError>,
) -> Result<Bytes, SelectionError> {
    match target.kind {
        Kind::Scalar(width) => Ok(Bytes::from(small(read(target.index)?, width)?)),
        Kind::Byte(offset) => Ok(Bytes::from(vec![read(target.index)?[offset]])),
        Kind::Absent => Err(SelectionError::AbsentField),
        _ => Err(SelectionError::CodecRequired),
    }
}

fn check_whole_verification_budget(
    kind: Kind,
    value: &[u8],
    limit: usize,
) -> Result<(), SelectionError> {
    let mut bytes = 0;
    add(
        &mut bytes,
        value.len().checked_mul(8).ok_or(SelectionError::LimitExceeded("source bytes"))?,
        limit,
        "source bytes",
    )?;
    add(&mut bytes, 4096, limit, "source bytes")?;
    whole_structure_budget(kind, value, &mut bytes, limit)
}

fn whole_root(kind: Kind, value: &[u8], limit: usize) -> Result<B256, SelectionError> {
    check_whole_verification_budget(kind, value, limit)?;
    if matches!(kind, Kind::Receipt(_) | Kind::Log | Kind::Receipts | Kind::Logs) {
        let receipts = match kind {
            Kind::Receipts => {
                Receipts::from_ssz_bytes(value).map_err(|_| SelectionError::InvalidValue)?
            }
            Kind::Receipt(selector) => {
                let receipt =
                    Receipt::from_ssz_bytes(value).map_err(|_| SelectionError::InvalidValue)?;
                if receipt.selector() != selector {
                    return Err(SelectionError::InvalidValue);
                }
                Receipts::new(vec![receipt])
            }
            Kind::Log | Kind::Logs => {
                let logs = if kind == Kind::Log {
                    vec![Log::from_ssz_bytes(value).map_err(|_| SelectionError::InvalidValue)?]
                } else {
                    Vec::<Log>::from_ssz_bytes(value).map_err(|_| SelectionError::InvalidValue)?
                };
                Receipts::new(vec![Receipt::Basic(BasicReceipt {
                    from_: alloy_primitives::Address::ZERO,
                    gas_used: 0,
                    logs,
                    status: false,
                })])
            }
            _ => return Err(SelectionError::InvalidValue),
        };
        let snapshot =
            Eip6466ReceiptSnapshot::build(receipts).map_err(|_| SelectionError::InvalidValue)?;
        if kind == Kind::Receipts {
            return Ok(snapshot.root());
        }
        let path = if kind == Kind::Log {
            "[0].logs[0]"
        } else if kind == Kind::Logs {
            "[0].logs"
        } else {
            "[0]"
        };
        let mut read = |index| {
            node_and_branch(snapshot.tree(), index)
                .map(|(node, _)| node)
                .map_err(|_| SelectionError::InvalidProof)
        };
        let target = resolve_eip(&tokens(path)?, &mut read)?;
        return read(target.index);
    }
    if kind == Kind::Data {
        return progressive_byte_list(value)
            .map(|node| node.root())
            .map_err(|_| SelectionError::InvalidValue);
    }
    let width = match kind {
        Kind::Topics => 32,
        Kind::Authorities => 20,
        _ => return Err(SelectionError::CodecRequired),
    };
    if !value.len().is_multiple_of(width) || (kind == Kind::Topics && value.len() / width > 4) {
        return Err(SelectionError::InvalidValue);
    }
    let nodes = value
        .chunks_exact(width)
        .map(|value| RetainedNode::leaf(B256::right_padding_from(value)))
        .collect();
    let contents =
        if kind == Kind::Topics { merkleize_fixed(nodes, 4) } else { merkleize_progressive(nodes) }
            .map_err(|_| SelectionError::InvalidValue)?;
    Ok(mix_in_length(contents, value.len() / width).root())
}

fn supplied_whole<'a>(
    snapshot: &'a Eip6466ReceiptSnapshot,
    path: &[PathToken],
) -> Result<&'a [u8], SelectionError> {
    if path.is_empty() {
        return Ok(snapshot.serialized());
    }
    let [PathToken::Index(receipt_index), rest @ ..] = path else {
        return Err(SelectionError::InvalidPath);
    };
    let receipt = variable_items(snapshot.serialized())?
        .nth(usize::try_from(*receipt_index).map_err(|_| SelectionError::OutOfBounds)?)
        .ok_or(SelectionError::OutOfBounds)??;
    if rest.is_empty() {
        return Ok(receipt);
    }
    let [PathToken::Field(field), rest @ ..] = rest else {
        return Err(SelectionError::InvalidPath);
    };
    let (logs, authorities) = receipt_parts(receipt)?;
    if field == "authorities" && rest.is_empty() {
        return authorities.ok_or(SelectionError::AbsentField);
    }
    if field != "logs" {
        return Err(SelectionError::InvalidPath);
    }
    if rest.is_empty() {
        return Ok(logs);
    }
    let [PathToken::Index(log_index), rest @ ..] = rest else {
        return Err(SelectionError::CodecRequired);
    };
    let log = variable_items(logs)?
        .nth(usize::try_from(*log_index).map_err(|_| SelectionError::OutOfBounds)?)
        .ok_or(SelectionError::OutOfBounds)??;
    if rest.is_empty() {
        return Ok(log);
    }
    let [PathToken::Field(log_field)] = rest else {
        return Err(SelectionError::CodecRequired);
    };
    let (topics, data) = log_parts(log)?;
    match log_field.as_str() {
        "data" => Ok(data),
        "topics" => Ok(topics),
        _ => Err(SelectionError::CodecRequired),
    }
}

fn variable_items(
    bytes: &[u8],
) -> Result<impl ExactSizeIterator<Item = Result<&[u8], SelectionError>>, SelectionError> {
    let fixed = if bytes.is_empty() {
        0
    } else {
        read_offset(bytes).map_err(|_| SelectionError::InvalidValue)?
    };
    if fixed > bytes.len() || !fixed.is_multiple_of(4) || (fixed == 0 && !bytes.is_empty()) {
        return Err(SelectionError::InvalidValue);
    }
    Ok((0..fixed / 4).map(move |index| {
        let start = read_offset(&bytes[index * 4..]).map_err(|_| SelectionError::InvalidValue)?;
        let end = if (index + 1) * 4 == fixed {
            bytes.len()
        } else {
            read_offset(&bytes[(index + 1) * 4..]).map_err(|_| SelectionError::InvalidValue)?
        };
        if start < fixed {
            return Err(SelectionError::InvalidValue);
        }
        bytes.get(start..end).ok_or(SelectionError::InvalidValue)
    }))
}

fn receipt_parts(value: &[u8]) -> Result<(&[u8], Option<&[u8]>), SelectionError> {
    let (&selector, body) = value.split_first().ok_or(SelectionError::InvalidValue)?;
    if !matches!(selector, 1..=3) {
        return Err(SelectionError::InvalidValue);
    }
    let decode = || -> Result<_, ssz::DecodeError> {
        let mut builder = SszDecoderBuilder::new(body);
        builder.register_type::<Address>()?;
        builder.register_type::<u64>()?;
        if selector == 2 {
            builder.register_type::<Address>()?;
        }
        builder.register_type::<Vec<Log>>()?;
        builder.register_type::<bool>()?;
        if selector == 3 {
            builder.register_type::<Vec<Address>>()?;
        }
        let mut decoder = builder.build()?;
        decoder.decode_next::<Address>()?;
        decoder.decode_next::<u64>()?;
        if selector == 2 {
            decoder.decode_next::<Address>()?;
        }
        let logs = decoder.decode_next_with(Ok)?;
        decoder.decode_next::<bool>()?;
        let authorities = if selector == 3 { Some(decoder.decode_next_with(Ok)?) } else { None };
        Ok((logs, authorities))
    };
    decode().map_err(|_| SelectionError::InvalidValue)
}

fn log_parts(value: &[u8]) -> Result<(&[u8], &[u8]), SelectionError> {
    let decode = || -> Result<_, ssz::DecodeError> {
        let mut builder = SszDecoderBuilder::new(value);
        builder.register_type::<Address>()?;
        builder.register_type::<Vec<B256>>()?;
        builder.register_type::<Bytes>()?;
        let mut decoder = builder.build()?;
        decoder.decode_next::<Address>()?;
        Ok((decoder.decode_next_with(Ok)?, decoder.decode_next_with(Ok)?))
    };
    decode().map_err(|_| SelectionError::InvalidValue)
}

fn progressive_budget(count: usize, bytes: &mut usize, limit: usize) -> Result<(), SelectionError> {
    let mut remaining = count;
    let mut width = 1_usize;
    let mut nodes = 3_usize;
    while remaining > 0 {
        nodes = nodes
            .checked_add(width.checked_mul(6).ok_or(SelectionError::LimitExceeded("source bytes"))?)
            .ok_or(SelectionError::LimitExceeded("source bytes"))?;
        remaining = remaining.saturating_sub(width);
        if remaining > 0 {
            width = width.checked_mul(4).ok_or(SelectionError::LimitExceeded("source bytes"))?;
        }
    }
    nodes = nodes
        .checked_add(count.checked_mul(2).ok_or(SelectionError::LimitExceeded("source bytes"))?)
        .ok_or(SelectionError::LimitExceeded("source bytes"))?;
    add(
        bytes,
        nodes
            .checked_mul(size_of::<RetainedNode>())
            .ok_or(SelectionError::LimitExceeded("source bytes"))?,
        limit,
        "source bytes",
    )
}

fn whole_structure_budget(
    kind: Kind,
    value: &[u8],
    bytes: &mut usize,
    limit: usize,
) -> Result<(), SelectionError> {
    match kind {
        Kind::Receipt(_) => {
            let (logs, authorities) = receipt_parts(value)?;
            add(bytes, 2 * size_of::<Receipt>(), limit, "source bytes")?;
            progressive_budget(if authorities.is_some() { 6 } else { 5 }, bytes, limit)?;
            whole_structure_budget(Kind::Logs, logs, bytes, limit)?;
            if let Some(authorities) = authorities {
                whole_structure_budget(Kind::Authorities, authorities, bytes, limit)?;
            }
        }
        Kind::Receipts | Kind::Logs => {
            let items = variable_items(value)?;
            progressive_budget(items.len(), bytes, limit)?;
            for item in items {
                whole_structure_budget(
                    if kind == Kind::Receipts { Kind::Receipt(0) } else { Kind::Log },
                    item?,
                    bytes,
                    limit,
                )?;
            }
        }
        Kind::Log => {
            let (topics, data) = log_parts(value)?;
            add(
                bytes,
                2 * size_of::<Log>() + 32 * size_of::<RetainedNode>(),
                limit,
                "source bytes",
            )?;
            whole_structure_budget(Kind::Topics, topics, bytes, limit)?;
            whole_structure_budget(Kind::Data, data, bytes, limit)?;
        }
        Kind::Data => progressive_budget(value.len().div_ceil(32), bytes, limit)?,
        Kind::Authorities => {
            if !value.len().is_multiple_of(20) {
                return Err(SelectionError::InvalidValue);
            }
            progressive_budget(value.len() / 20, bytes, limit)?;
        }
        Kind::Topics => {
            if !value.len().is_multiple_of(32) || value.len() > 128 {
                return Err(SelectionError::InvalidValue);
            }
            add(bytes, 32 * size_of::<RetainedNode>(), limit, "source bytes")?;
        }
        _ => return Err(SelectionError::CodecRequired),
    }
    Ok(())
}

fn add(
    count: &mut usize,
    amount: usize,
    limit: usize,
    name: &'static str,
) -> Result<(), SelectionError> {
    *count = count.checked_add(amount).ok_or(SelectionError::LimitExceeded(name))?;
    if *count > limit {
        return Err(SelectionError::LimitExceeded(name));
    }
    Ok(())
}

fn remaining_materialization(
    source: usize,
    values: usize,
    hashes: usize,
    limit: usize,
) -> Result<usize, SelectionError> {
    let used = values
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(source))
        .and_then(|bytes| bytes.checked_add(hashes.checked_mul(64)?))
        .ok_or(SelectionError::LimitExceeded("source bytes"))?;
    limit.checked_sub(used).ok_or(SelectionError::LimitExceeded("source bytes"))
}

fn evaluate(
    selection: &ReceiptSelection,
    read: &mut impl FnMut(u128) -> Result<B256, SelectionError>,
    whole: Option<&[Bytes]>,
    verify_whole: bool,
    claims: &mut Vec<Vec<PathToken>>,
    targets: &mut usize,
    limits: SelectionLimits,
) -> Result<Vec<Bytes>, SelectionError> {
    let path = tokens(&selection.path)?;
    let target = resolve_eip(&path, read)?;
    let mut selected = Vec::new();
    match selection.operation {
        SelectionOperation::Range { start, end } | SelectionOperation::Slice { start, end } => {
            let length = target.length.ok_or(SelectionError::UnsupportedOperation)?;
            if start > end || end > length {
                return Err(SelectionError::OutOfBounds);
            }
            if matches!(selection.operation, SelectionOperation::Slice { .. }) &&
                target.kind != Kind::Data
            {
                return Err(SelectionError::UnsupportedOperation);
            }
            let count = usize::try_from(end - start)
                .map_err(|_| SelectionError::LimitExceeded("targets"))?;
            add(targets, count.max(1), limits.targets, "targets")?;
            if target.kind == Kind::Data {
                check_padding(target, read)?;
            }
            for index in start..end {
                let mut child = path.clone();
                child.push(PathToken::Index(index));
                register(claims, child.clone())?;
                let child_target = resolve_eip(&child, read)?;
                if matches!(child_target.kind, Kind::Receipt(_) | Kind::Log) {
                    let value = whole
                        .and_then(|values| values.get(selected.len()))
                        .ok_or(SelectionError::CodecRequired)?;
                    let node = read(child_target.index)?;
                    if verify_whole &&
                        whole_root(child_target.kind, value, limits.source_bytes)? != node
                    {
                        return Err(SelectionError::InvalidValue);
                    }
                    selected.push(value.clone());
                } else {
                    selected.push(claim_value(child_target, read)?);
                }
            }
            if matches!(selection.operation, SelectionOperation::Slice { .. }) {
                selected = vec![Bytes::from(
                    selected.iter().flat_map(|value| value.iter().copied()).collect::<Vec<_>>(),
                )];
            }
        }
        SelectionOperation::Whole {} | SelectionOperation::Value {}
            if matches!(
                target.kind,
                Kind::Data |
                    Kind::Topics |
                    Kind::Authorities |
                    Kind::Receipt(_) |
                    Kind::Log |
                    Kind::Receipts |
                    Kind::Logs
            ) =>
        {
            add(targets, 1, limits.targets, "targets")?;
            register(claims, path)?;
            let value =
                whole.and_then(|values| values.first()).ok_or(SelectionError::CodecRequired)?;
            if value.len() > limits.response_bytes {
                return Err(SelectionError::LimitExceeded("value bytes"));
            }
            let unit = if target.kind == Kind::Authorities {
                20
            } else if target.kind == Kind::Topics {
                32
            } else {
                1
            };
            if matches!(target.kind, Kind::Data | Kind::Topics | Kind::Authorities) &&
                (value.len() % unit != 0 ||
                    value.len() / unit !=
                        usize::try_from(target.length.unwrap_or(0))
                            .map_err(|_| SelectionError::InvalidValue)?)
            {
                return Err(SelectionError::InvalidValue);
            }
            let node = read(target.index)?;
            if verify_whole && whole_root(target.kind, value, limits.source_bytes)? != node {
                return Err(SelectionError::InvalidValue);
            }
            selected.push(Bytes::copy_from_slice(value));
        }
        SelectionOperation::Value {} | SelectionOperation::Whole {} => {
            add(targets, 1, limits.targets, "targets")?;
            register(claims, path)?;
            let value = claim_value(target, read)?;
            if matches!(target.kind, Kind::Scalar(1)) && value[0] > 1 {
                return Err(SelectionError::InvalidValue);
            }
            selected.push(value);
        }
        SelectionOperation::Length {} => {
            add(targets, 1, limits.targets, "targets")?;
            selected.push(Bytes::from(
                target.length.ok_or(SelectionError::UnsupportedOperation)?.to_le_bytes().to_vec(),
            ));
        }
        SelectionOperation::Variant {} => {
            add(targets, 1, limits.targets, "targets")?;
            let Kind::Receipt(selector) = target.kind else {
                return Err(SelectionError::UnsupportedOperation);
            };
            selected.push(Bytes::from(vec![selector]));
        }
        SelectionOperation::Presence {} => {
            add(targets, 1, limits.targets, "targets")?;
            if !matches!(path.last(), Some(PathToken::Field(_))) {
                return Err(SelectionError::UnsupportedOperation);
            }
            selected.push(Bytes::from(vec![u8::from(target.kind != Kind::Absent)]));
        }
    }
    Ok(selected)
}

fn register(claims: &mut Vec<Vec<PathToken>>, path: Vec<PathToken>) -> Result<(), SelectionError> {
    for previous in claims.iter() {
        if path.starts_with(previous) || previous.starts_with(&path) {
            return Err(SelectionError::DuplicateOrOverlap);
        }
    }
    claims.push(path);
    Ok(())
}

pub(crate) fn check_request(
    request: &SelectionRequest,
    limits: SelectionLimits,
) -> Result<(), SelectionError> {
    if request.selections.is_empty() || request.selections.len() > limits.selections {
        return Err(SelectionError::LimitExceeded("selections"));
    }
    if request.selections.iter().any(|selection| selection.path.len() > 256) {
        return Err(SelectionError::InvalidPath);
    }
    encoded_size(request, limits.request_bytes, "request bytes")?;
    let mut targets = 0;
    for (index, selection) in request.selections.iter().enumerate() {
        let path = tokens(&selection.path)?;
        let count = match selection.operation {
            SelectionOperation::Range { start, end } | SelectionOperation::Slice { start, end } => {
                let count = end.checked_sub(start).ok_or(SelectionError::OutOfBounds)?;
                usize::try_from(count.max(1))
                    .map_err(|_| SelectionError::LimitExceeded("targets"))?
            }
            _ => 1,
        };
        add(&mut targets, count, limits.targets, "targets")?;
        for previous in &request.selections[..index] {
            if tokens(&previous.path)? == path {
                let bounds = |operation: &SelectionOperation| match operation {
                    SelectionOperation::Range { start, end } |
                    SelectionOperation::Slice { start, end } => Some((*start, *end)),
                    _ => None,
                };
                let disjoint = match (bounds(&previous.operation), bounds(&selection.operation)) {
                    (Some((a, b)), Some((c, d))) => a < b && c < d && (b <= c || d <= a),
                    _ => false,
                };
                if !disjoint {
                    return Err(SelectionError::DuplicateOrOverlap);
                }
            }
        }
    }
    Ok(())
}

fn source_budget(
    snapshot: &Eip6466ReceiptSnapshot,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<usize, SelectionError> {
    if snapshot.receipts().len() > limits.receipts {
        return Err(SelectionError::LimitExceeded("receipts"));
    }
    let mut logs = 0;
    let mut bytes = snapshot.serialized().len();
    let mut pending = vec![(snapshot.tree(), 0_u32)];
    while let Some((node, depth)) = pending.pop() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(SelectionError::Cancelled);
        }
        if depth > 127 {
            return Err(SelectionError::LimitExceeded("proof depth"));
        }
        add(&mut bytes, size_of::<RetainedNode>(), limits.source_bytes, "source bytes")?;
        if let Some(children) = node.children() {
            pending.extend(children.iter().map(|child| (child, depth + 1)));
        }
    }
    for receipt in snapshot.receipts().as_slice() {
        add(&mut bytes, size_of::<Receipt>(), limits.source_bytes, "source bytes")?;
        let receipt_logs = receipt.logs();
        add(&mut logs, receipt_logs.len(), limits.logs, "logs")?;
        for log in receipt_logs {
            add(&mut bytes, size_of_val(log), limits.source_bytes, "source bytes")?;
            add(&mut bytes, log.data().len(), limits.source_bytes, "source bytes")?;
            add(
                &mut bytes,
                log.topics()
                    .len()
                    .checked_mul(32)
                    .ok_or(SelectionError::LimitExceeded("source bytes"))?,
                limits.source_bytes,
                "source bytes",
            )?;
        }
        if let Receipt::SetCode(receipt) = receipt {
            add(
                &mut bytes,
                receipt
                    .authorities
                    .len()
                    .checked_mul(20)
                    .ok_or(SelectionError::LimitExceeded("source bytes"))?,
                limits.source_bytes,
                "source bytes",
            )?;
        }
    }
    Ok(bytes)
}

pub fn select_receipt_snapshot(
    snapshot: &Eip6466ReceiptSnapshot,
    request: &SelectionRequest,
    limits: SelectionLimits,
) -> Result<SelectionResponse, SelectionError> {
    select_with_cancel(snapshot, request, limits, &AtomicBool::new(false))
}

pub(crate) fn select_with_cancel(
    snapshot: &Eip6466ReceiptSnapshot,
    request: &SelectionRequest,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<SelectionResponse, SelectionError> {
    check_request(request, limits)?;
    let source_bytes = source_budget(snapshot, limits, cancelled)?;
    let mut results = Vec::new();
    let mut claims = Vec::new();
    let mut targets = 0;
    let hashes = Cell::new(0);
    let mut value_bytes = 0;
    for selection in &request.selections {
        let mut witnesses = Vec::new();
        let mut cache = BTreeMap::new();
        let mut read = |index| {
            if cancelled.load(Ordering::Relaxed) {
                return Err(SelectionError::Cancelled);
            }
            if let Some(node) = cache.get(&index) {
                return Ok(*node);
            }
            let (node, branch) = node_and_branch(snapshot.tree(), index)
                .map_err(|_| SelectionError::InvalidProof)?;
            if request.include_proof {
                let mut count = hashes.get();
                add(&mut count, branch.len() + 1, limits.witness_hashes, "witness hashes")?;
                hashes.set(count);
                witnesses.push(SelectionWitness { node, branch });
            }
            cache.insert(index, node);
            Ok(node)
        };
        let path = tokens(&selection.path)?;
        let target = resolve_eip(&path, &mut read)?;
        let mut whole = None;
        let mut paths = Vec::new();
        if matches!(
            selection.operation,
            SelectionOperation::Value {} | SelectionOperation::Whole {}
        ) && matches!(
            target.kind,
            Kind::Data |
                Kind::Topics |
                Kind::Authorities |
                Kind::Receipt(_) |
                Kind::Log |
                Kind::Receipts |
                Kind::Logs
        ) {
            paths.push(path.clone());
        } else if let SelectionOperation::Range { start, end } = selection.operation {
            if start > end || end > target.length.unwrap_or(0) {
                return Err(SelectionError::OutOfBounds);
            }
            if matches!(target.kind, Kind::Receipts | Kind::Logs) {
                if end - start > limits.targets.saturating_sub(targets) as u64 {
                    return Err(SelectionError::LimitExceeded("targets"));
                }
                for index in start..end {
                    let mut child = path.clone();
                    child.push(PathToken::Index(index));
                    paths.push(child);
                }
            }
        }
        if !paths.is_empty() {
            let mut selected_bytes = 0;
            let mut borrowed = Vec::with_capacity(paths.len());
            for path in &paths {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(SelectionError::Cancelled);
                }
                let value = supplied_whole(snapshot, path)?;
                add(
                    &mut selected_bytes,
                    value.len(),
                    limits.response_bytes.saturating_sub(value_bytes),
                    "value bytes",
                )?;
                borrowed.push(value);
            }
            let available = remaining_materialization(
                source_bytes,
                value_bytes,
                hashes.get(),
                limits.source_bytes,
            )?;
            let available = selected_bytes
                .checked_mul(2)
                .and_then(|bytes| available.checked_sub(bytes))
                .ok_or(SelectionError::LimitExceeded("source bytes"))?;
            let mut values = Vec::with_capacity(paths.len());
            for value in borrowed {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(SelectionError::Cancelled);
                }
                let kind = match (&selection.operation, target.kind) {
                    (SelectionOperation::Range { .. }, Kind::Receipts) => {
                        Kind::Receipt(*value.first().ok_or(SelectionError::InvalidValue)?)
                    }
                    (SelectionOperation::Range { .. }, Kind::Logs) => Kind::Log,
                    (_, kind) => kind,
                };
                check_whole_verification_budget(kind, value, available)?;
                values.push(Bytes::copy_from_slice(value));
            }
            whole = Some(values);
        }
        let remaining = SelectionLimits {
            source_bytes: remaining_materialization(
                source_bytes,
                value_bytes,
                hashes.get(),
                limits.source_bytes,
            )?,
            ..limits
        };
        let values = evaluate(
            selection,
            &mut read,
            whole.as_deref(),
            false,
            &mut claims,
            &mut targets,
            remaining,
        )?;
        for value in &values {
            add(&mut value_bytes, value.len(), limits.response_bytes, "value bytes")?;
        }
        remaining_materialization(source_bytes, value_bytes, hashes.get(), limits.source_bytes)?;
        results.push(SelectionResult {
            values_ssz: values,
            witnesses: request.include_proof.then_some(witnesses),
        });
    }
    let response = SelectionResponse {
        root: snapshot.root(),
        proof_format: request.include_proof.then_some(SelectionProofFormat::SingleBranch),
        results,
    };
    encoded_size(&response, limits.response_bytes, "response bytes")?;
    Ok(response)
}

pub fn verify_receipt_selection(
    request: &SelectionRequest,
    response: &SelectionResponse,
    expected_root: B256,
    limits: SelectionLimits,
) -> Result<(), SelectionError> {
    check_request(request, limits)?;
    if !request.include_proof {
        return Err(SelectionError::Unverified);
    }
    if response.root != expected_root ||
        response.proof_format != Some(SelectionProofFormat::SingleBranch) ||
        response.results.len() != request.selections.len()
    {
        return Err(SelectionError::InvalidContext);
    }
    encoded_size(response, limits.response_bytes, "response bytes")?;
    let mut claims = Vec::new();
    let mut targets = 0;
    let mut hashes = 0;
    for (selection, result) in request.selections.iter().zip(&response.results) {
        let witnesses = result.witnesses.as_ref().ok_or(SelectionError::InvalidProof)?;
        let mut remaining = witnesses.iter();
        let mut cache = BTreeMap::new();
        let mut read = |index| {
            if let Some(node) = cache.get(&index) {
                return Ok(*node);
            }
            let witness = remaining.next().ok_or(SelectionError::InvalidProof)?;
            add(&mut hashes, witness.branch.len() + 1, limits.witness_hashes, "witness hashes")?;
            verify_branch(witness.node, index, &witness.branch, expected_root)
                .map_err(|_| SelectionError::InvalidProof)?;
            cache.insert(index, witness.node);
            Ok(witness.node)
        };
        resolve_eip(&tokens(&selection.path)?, &mut read)?;
        let values = evaluate(
            selection,
            &mut read,
            Some(result.values_ssz.as_slice()),
            true,
            &mut claims,
            &mut targets,
            limits,
        )?;
        if values != result.values_ssz {
            return Err(SelectionError::InvalidValue);
        }
        if remaining.next().is_some() {
            return Err(SelectionError::InvalidProof);
        }
    }
    Ok(())
}

pub fn decode_selection_request(
    bytes: &[u8],
    limits: SelectionLimits,
) -> Result<SelectionRequest, SelectionError> {
    if bytes.len() > limits.request_bytes {
        return Err(SelectionError::LimitExceeded("request bytes"));
    }
    let request = serde_json::from_slice(bytes).map_err(|_| SelectionError::InvalidValue)?;
    check_request(&request, limits)?;
    Ok(request)
}

struct JsonCounter {
    bytes: usize,
    limit: usize,
}

impl Write for JsonCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self
            .bytes
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("JSON size overflow"))?;
        if count > self.limit {
            return Err(io::Error::other("JSON size limit"));
        }
        self.bytes = count;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn encoded_size(
    value: &impl Serialize,
    limit: usize,
    name: &'static str,
) -> Result<usize, SelectionError> {
    let mut counter = JsonCounter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|_| SelectionError::LimitExceeded(name))?;
    Ok(counter.bytes)
}

#[cfg(test)]
mod tests;
