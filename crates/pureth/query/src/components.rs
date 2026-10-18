use crate::{
    compose_gindices, container_field_gindex, progressive_chunk_gindex,
    proof_access::node_at,
    selection::{add, check_request, encoded_size, multiproof, remaining_materialization, tokens},
    PathToken, ReceiptSelection, SelectionError, SelectionLimits, SelectionOperation,
    SelectionProofFormat, SelectionRequest, SelectionResponse, SelectionResult,
};
use alloy_primitives::{Bytes, B256};
use pureth_withdrawal_root::WithdrawalSnapshot;
use reth_pureth_block::{
    BlockAccessListSnapshot, ExecutionPayloadSnapshot, ExecutionRequestsSnapshot,
    ExecutionSummarySnapshot,
};
use reth_pureth_ssz::{merkleize_fixed, merkleize_progressive, mix_in_length, RetainedNode};
use reth_pureth_transaction::TransactionSnapshot;
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    collections::BTreeMap,
    mem::size_of,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentObject {
    BlockAccessList,
    Transactions,
    Withdrawals,
    ExecutionRequests,
    ExecutionSummary,
}

#[derive(Clone, Copy, Debug)]
pub enum ComponentSnapshot<'a> {
    BlockAccessList(&'a BlockAccessListSnapshot),
    Transactions(&'a TransactionSnapshot),
    Withdrawals(&'a WithdrawalSnapshot),
    ExecutionRequests(&'a ExecutionRequestsSnapshot),
    ExecutionSummary(&'a ExecutionSummarySnapshot),
}

impl<'a> ComponentSnapshot<'a> {
    pub const fn from_payload(
        payload: &'a ExecutionPayloadSnapshot,
        object: ComponentObject,
    ) -> Self {
        let components = payload.components();
        match object {
            ComponentObject::BlockAccessList => {
                Self::BlockAccessList(&components.block_access_list)
            }
            ComponentObject::Transactions => Self::Transactions(&components.transactions),
            ComponentObject::Withdrawals => Self::Withdrawals(&components.withdrawals),
            ComponentObject::ExecutionRequests => Self::ExecutionRequests(&components.requests),
            ComponentObject::ExecutionSummary => Self::ExecutionSummary(payload.summary()),
        }
    }

    pub const fn object(self) -> ComponentObject {
        match self {
            Self::BlockAccessList(_) => ComponentObject::BlockAccessList,
            Self::Transactions(_) => ComponentObject::Transactions,
            Self::Withdrawals(_) => ComponentObject::Withdrawals,
            Self::ExecutionRequests(_) => ComponentObject::ExecutionRequests,
            Self::ExecutionSummary(_) => ComponentObject::ExecutionSummary,
        }
    }
    pub const fn tree(self) -> &'a RetainedNode {
        match self {
            Self::BlockAccessList(value) => value.tree(),
            Self::Transactions(value) => value.tree(),
            Self::Withdrawals(value) => value.tree(),
            Self::ExecutionRequests(value) => value.tree(),
            Self::ExecutionSummary(value) => value.tree(),
        }
    }
    pub fn serialized(self) -> &'a [u8] {
        match self {
            Self::BlockAccessList(value) => value.serialized(),
            Self::Transactions(value) => value.serialized(),
            Self::Withdrawals(value) => value.serialized(),
            Self::ExecutionRequests(value) => value.serialized(),
            Self::ExecutionSummary(value) => value.serialized(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentResponse {
    pub object: ComponentObject,
    pub selection: SelectionResponse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentLinkProof {
    pub component: ComponentObject,
    pub summary: ComponentResponse,
}

fn link_request(object: ComponentObject) -> Result<SelectionRequest, SelectionError> {
    let path = match object {
        ComponentObject::BlockAccessList => ".block_access_list_root",
        ComponentObject::Transactions => ".transactions_root",
        ComponentObject::Withdrawals => ".withdrawals_root",
        ComponentObject::ExecutionRequests => ".requests_root",
        ComponentObject::ExecutionSummary => return Err(SelectionError::UnsupportedOperation),
    };
    Ok(SelectionRequest {
        include_proof: true,
        selections: vec![ReceiptSelection {
            path: path.into(),
            operation: SelectionOperation::Value {},
        }],
    })
}

pub fn link_component(
    component: ComponentSnapshot<'_>,
    summary: &ExecutionSummarySnapshot,
    limits: SelectionLimits,
) -> Result<ComponentLinkProof, SelectionError> {
    let request = link_request(component.object())?;
    let response =
        select_component(ComponentSnapshot::ExecutionSummary(summary), &request, limits)?;
    if response.selection.results[0].values_ssz[0].as_ref() != component.tree().root().as_slice() {
        return Err(SelectionError::InvalidContext);
    }
    let proof = ComponentLinkProof { component: component.object(), summary: response };
    encoded_size(&proof, limits.response_bytes, "response bytes")?;
    Ok(proof)
}

pub fn verify_component_link(
    object: ComponentObject,
    component_root: B256,
    proof: &ComponentLinkProof,
    expected_summary_root: B256,
    limits: SelectionLimits,
) -> Result<(), SelectionError> {
    encoded_size(proof, limits.response_bytes, "response bytes")?;
    if proof.component != object {
        return Err(SelectionError::InvalidContext);
    }
    verify_component_selection(
        ComponentObject::ExecutionSummary,
        &link_request(object)?,
        &proof.summary,
        expected_summary_root,
        limits,
    )?;
    if proof.summary.selection.results[0].values_ssz[0].as_ref() != component_root.as_slice() {
        return Err(SelectionError::InvalidContext);
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Record {
    Transaction,
    Payload(u8),
    Authorization,
    AuthorizationPayload(u8),
    AccessTuple,
    Hash,
    BasicFees,
    Withdrawal,
    Deposit,
    WithdrawalRequest,
    Consolidation,
    Requests,
    Summary,
    Gas,
    Fees,
}

#[derive(Clone, Copy)]
enum Kind {
    Inactive,
    Union { authorization: bool },
    ProgressiveBytes,
    ProgressiveByte { position: usize, length: usize },
    Scalar(usize),
    Bytes(usize),
    Byte { position: usize, width: usize },
    ByteList,
    Record(Record),
    List(Record, usize),
}

#[derive(Clone, Copy)]
struct Target {
    index: u128,
    kind: Kind,
    length: Option<u64>,
}

impl ComponentObject {
    const fn kind(self) -> Kind {
        match self {
            Self::BlockAccessList => Kind::ProgressiveBytes,
            Self::Transactions => Kind::List(Record::Transaction, 0),
            Self::Withdrawals => Kind::List(Record::Withdrawal, 0),
            Self::ExecutionRequests => Kind::Record(Record::Requests),
            Self::ExecutionSummary => Kind::Record(Record::Summary),
        }
    }
}

impl Record {
    fn fields(self) -> Vec<(&'static str, Kind)> {
        match self {
            Self::Transaction => vec![
                ("payload", Kind::Union { authorization: false }),
                ("signature", Kind::ProgressiveBytes),
            ],
            Self::Authorization => vec![
                ("payload", Kind::Union { authorization: true }),
                ("signature", Kind::ProgressiveBytes),
            ],
            Self::AuthorizationPayload(selector) => vec![
                ("magic", Kind::Scalar(1)),
                ("chain_id", if selector == 1 { Kind::Inactive } else { Kind::Scalar(32) }),
                ("address", Kind::Bytes(20)),
                ("nonce", Kind::Scalar(8)),
            ],
            Self::AccessTuple => {
                vec![("address", Kind::Bytes(20)), ("storage_keys", Kind::List(Self::Hash, 0))]
            }
            Self::Hash => vec![("", Kind::Bytes(32))],
            Self::BasicFees => vec![("regular", Kind::Scalar(32))],
            Self::Payload(selector) => {
                let mut fields = vec![
                    ("type_", Kind::Scalar(1)),
                    ("chain_id", if selector <= 2 { Kind::Inactive } else { Kind::Scalar(32) }),
                    ("nonce", Kind::Scalar(8)),
                    (
                        "max_fees_per_gas",
                        Kind::Record(if selector == 9 { Self::Fees } else { Self::BasicFees }),
                    ),
                    ("gas", Kind::Scalar(8)),
                    (
                        "to",
                        if matches!(selector, 2 | 4 | 6 | 8) {
                            Kind::Inactive
                        } else {
                            Kind::Bytes(20)
                        },
                    ),
                    ("value", Kind::Scalar(32)),
                    ("input_", Kind::ProgressiveBytes),
                ];
                if selector >= 5 {
                    fields.push(("access_list", Kind::List(Self::AccessTuple, 0)));
                }
                if selector >= 7 {
                    fields.push(("max_priority_fees_per_gas", Kind::Record(Self::BasicFees)));
                }
                if selector == 9 {
                    fields.push(("blob_versioned_hashes", Kind::List(Self::Hash, 0)));
                }
                if selector == 10 {
                    fields.push(("blob_versioned_hashes", Kind::Inactive));
                    fields.push(("authorization_list", Kind::List(Self::Authorization, 0)));
                }
                for name in [
                    "access_list",
                    "max_priority_fees_per_gas",
                    "blob_versioned_hashes",
                    "authorization_list",
                ]
                .into_iter()
                .skip(fields.len() - 8)
                {
                    fields.push((name, Kind::Inactive));
                }
                fields
            }
            Self::Withdrawal => vec![
                ("index", Kind::Scalar(8)),
                ("validator_index", Kind::Scalar(8)),
                ("address", Kind::Bytes(20)),
                ("amount", Kind::Scalar(8)),
            ],
            Self::Deposit => vec![
                ("pubkey", Kind::Bytes(48)),
                ("withdrawal_credentials", Kind::Bytes(32)),
                ("amount", Kind::Scalar(8)),
                ("signature", Kind::Bytes(96)),
                ("index", Kind::Scalar(8)),
            ],
            Self::WithdrawalRequest => vec![
                ("source_address", Kind::Bytes(20)),
                ("validator_pubkey", Kind::Bytes(48)),
                ("amount", Kind::Scalar(8)),
            ],
            Self::Consolidation => vec![
                ("source_address", Kind::Bytes(20)),
                ("source_pubkey", Kind::Bytes(48)),
                ("target_pubkey", Kind::Bytes(48)),
            ],
            Self::Requests => vec![
                ("deposits", Kind::List(Self::Deposit, 8192)),
                ("withdrawals", Kind::List(Self::WithdrawalRequest, 16)),
                ("consolidations", Kind::List(Self::Consolidation, 2)),
            ],
            Self::Gas => vec![("regular", Kind::Scalar(8)), ("blob", Kind::Scalar(8))],
            Self::Fees => vec![("regular", Kind::Scalar(32)), ("blob", Kind::Scalar(32))],
            Self::Summary => vec![
                ("parent_hash", Kind::Bytes(32)),
                ("miner", Kind::Bytes(20)),
                ("state_root", Kind::Bytes(32)),
                ("transactions_root", Kind::Bytes(32)),
                ("receipts_root", Kind::Bytes(32)),
                ("number", Kind::Scalar(8)),
                ("gas_limits", Kind::Record(Self::Gas)),
                ("gas_used", Kind::Record(Self::Gas)),
                ("timestamp", Kind::Scalar(8)),
                ("extra_data", Kind::ByteList),
                ("mix_hash", Kind::Bytes(32)),
                ("base_fees_per_gas", Kind::Record(Self::Fees)),
                ("withdrawals_root", Kind::Bytes(32)),
                ("excess_gas", Kind::Record(Self::Gas)),
                ("parent_beacon_block_root", Kind::Bytes(32)),
                ("requests_root", Kind::Bytes(32)),
                ("block_access_list_root", Kind::Bytes(32)),
                ("slot_number", Kind::Scalar(8)),
            ],
        }
    }
    const fn mask(self) -> Option<u32> {
        match self {
            Self::BasicFees => Some(1),
            Self::Payload(selector) => Some(match selector {
                1 => 0xfd,
                2 => 0xdd,
                3 => 0xff,
                4 => 0xdf,
                5 => 0x1ff,
                6 => 0x1df,
                7 => 0x3ff,
                8 => 0x3df,
                9 => 0x7ff,
                10 => 0xbff,
                _ => 0,
            }),
            Self::AuthorizationPayload(selector) => Some(if selector == 1 { 0x0d } else { 0x0f }),
            Self::Withdrawal => Some(15),
            Self::Summary => Some(0x03ffff),
            Self::Gas | Self::Fees => Some(3),
            _ => None,
        }
    }
    fn width(self) -> usize {
        match self {
            Self::Hash | Self::BasicFees => 32,
            Self::Transaction | Self::Authorization => 8,
            Self::AccessTuple => 24,
            Self::Payload(_) | Self::AuthorizationPayload(_) => self
                .fields()
                .into_iter()
                .map(|(_, kind)| match kind {
                    Kind::Inactive => 0,
                    Kind::Scalar(width) | Kind::Bytes(width) => width,
                    Kind::Record(record) if !record.variable() => record.width(),
                    _ => 4,
                })
                .sum(),
            Self::Withdrawal => 44,
            Self::Deposit => 192,
            Self::WithdrawalRequest => 76,
            Self::Consolidation => 116,
            Self::Gas => 16,
            Self::Fees => 64,
            Self::Requests => 12,
            Self::Summary => 448,
        }
    }

    const fn variable(self) -> bool {
        matches!(
            self,
            Self::Transaction | Self::Authorization | Self::AccessTuple | Self::Payload(_)
        )
    }
}

const fn union_record(authorization: bool, selector: u8) -> Result<Record, SelectionError> {
    match (authorization, selector) {
        (false, 1..=10) => Ok(Record::Payload(selector)),
        (true, 1..=2) => Ok(Record::AuthorizationPayload(selector)),
        _ => Err(SelectionError::InvalidContext),
    }
}

fn list_count(record: Record, bytes: &[u8]) -> Result<usize, SelectionError> {
    if !record.variable() {
        return if bytes.len().is_multiple_of(record.width()) {
            Ok(bytes.len() / record.width())
        } else {
            Err(SelectionError::InvalidValue)
        };
    }
    if bytes.is_empty() {
        return Ok(0);
    }
    let offset = bytes.get(..4).ok_or(SelectionError::InvalidValue)?;
    let offset =
        u32::from_le_bytes(offset.try_into().map_err(|_| SelectionError::InvalidValue)?) as usize;
    if offset == 0 || !offset.is_multiple_of(4) || offset > bytes.len() {
        return Err(SelectionError::InvalidValue);
    }
    Ok(offset / 4)
}

fn list_item(record: Record, bytes: &[u8], index: usize) -> Result<&[u8], SelectionError> {
    let count = list_count(record, bytes)?;
    if index >= count {
        return Err(SelectionError::OutOfBounds);
    }
    if !record.variable() {
        let offset = index.checked_mul(record.width()).ok_or(SelectionError::OutOfBounds)?;
        return bytes.get(offset..offset + record.width()).ok_or(SelectionError::OutOfBounds);
    }
    let offset = |index: usize| -> Result<usize, SelectionError> {
        let start = index.checked_mul(4).ok_or(SelectionError::InvalidValue)?;
        Ok(u32::from_le_bytes(
            bytes
                .get(start..start + 4)
                .ok_or(SelectionError::InvalidValue)?
                .try_into()
                .map_err(|_| SelectionError::InvalidValue)?,
        ) as usize)
    };
    let start = offset(index)?;
    let end = if index + 1 == count { bytes.len() } else { offset(index + 1)? };
    if start < count * 4 || start >= end {
        return Err(SelectionError::InvalidValue);
    }
    bytes.get(start..end).ok_or(SelectionError::InvalidValue)
}

fn join(parent: u128, child: u128) -> Result<u128, SelectionError> {
    compose_gindices(parent, child).map_err(|_| SelectionError::LimitExceeded("proof depth"))
}

fn packed(node: B256, width: usize) -> Result<Bytes, SelectionError> {
    if node[width..].iter().any(|byte| *byte != 0) {
        return Err(SelectionError::InvalidValue);
    }
    Ok(Bytes::copy_from_slice(&node[..width]))
}

fn resolve(
    object: ComponentObject,
    path: &[PathToken],
    read: &mut impl FnMut(u128) -> Result<B256, SelectionError>,
    limits: SelectionLimits,
) -> Result<Target, SelectionError> {
    let mut target = Target { index: 1, kind: object.kind(), length: None };
    for (step, token) in path.iter().map(Some).chain(std::iter::once(None)).enumerate() {
        if let Kind::Union { authorization } = target.kind {
            let selector = packed(read(join(target.index, 3)?)?, 1)?[0];
            let record = union_record(authorization, selector)?;
            let payload = join(target.index, 2)?;
            let mask = record.mask().ok_or(SelectionError::InvalidContext)?;
            if read(join(payload, 3)?)? != B256::right_padding_from(&mask.to_le_bytes()) {
                return Err(SelectionError::InvalidContext);
            }
            let expected_type = if authorization {
                5
            } else {
                match selector {
                    1..=4 => 0,
                    5..=6 => 1,
                    7..=8 => 2,
                    9 => 3,
                    _ => 4,
                }
            };
            if packed(
                read(join(
                    payload,
                    progressive_chunk_gindex(0).map_err(|_| SelectionError::InvalidContext)?,
                )?)?,
                1,
            )?[0] !=
                expected_type
            {
                return Err(SelectionError::InvalidContext);
            }
            if authorization &&
                selector == 2 &&
                read(join(
                    payload,
                    progressive_chunk_gindex(1).map_err(|_| SelectionError::InvalidContext)?,
                )?)? == B256::ZERO
            {
                return Err(SelectionError::InvalidContext);
            }
            if token.is_some() {
                target = Target { index: payload, kind: Kind::Record(record), length: None };
            }
        }
        match target.kind {
            Kind::Record(record) => {
                if let Some(mask) = record.mask() &&
                    read(join(target.index, 3)?)? !=
                        B256::right_padding_from(&mask.to_le_bytes())
                {
                    return Err(SelectionError::InvalidContext);
                }
                if let Some(token) = token {
                    let PathToken::Field(name) = token else {
                        return Err(SelectionError::InvalidPath);
                    };
                    let fields = record.fields();
                    let (position, (_, kind)) = fields
                        .iter()
                        .enumerate()
                        .find(|(_, (field, _))| *field == name)
                        .ok_or(SelectionError::InvalidPath)?;
                    let child = if record.mask().is_some() {
                        progressive_chunk_gindex(position as u64)
                    } else {
                        container_field_gindex(fields.len(), position)
                    }
                    .map_err(|_| SelectionError::InvalidContext)?;
                    target =
                        Target { index: join(target.index, child)?, kind: *kind, length: None };
                }
            }
            Kind::List(record, bound) => {
                let bytes = packed(read(join(target.index, 3)?)?, 8)?;
                let count = u64::from_le_bytes(
                    bytes.as_ref().try_into().map_err(|_| SelectionError::InvalidValue)?,
                );
                let cap = if bound == 0 { usize::MAX } else { bound };
                if u128::from(count) > cap as u128 {
                    return Err(SelectionError::InvalidContext);
                }
                target.length = Some(count);
                if let Some(token) = token {
                    let PathToken::Index(index) = token else {
                        return Err(SelectionError::InvalidPath);
                    };
                    if *index >= count {
                        return Err(SelectionError::OutOfBounds);
                    }
                    let child = if bound == 0 {
                        progressive_chunk_gindex(*index)
                            .map_err(|_| SelectionError::InvalidContext)?
                    } else {
                        2 * bound as u128 + u128::from(*index)
                    };
                    target = Target {
                        index: join(target.index, child)?,
                        kind: if record == Record::Hash {
                            Kind::Bytes(32)
                        } else {
                            Kind::Record(record)
                        },
                        length: None,
                    };
                }
            }
            Kind::ProgressiveBytes => {
                let bytes = packed(read(join(target.index, 3)?)?, 8)?;
                let count = u64::from_le_bytes(
                    bytes.as_ref().try_into().map_err(|_| SelectionError::InvalidValue)?,
                );
                if u128::from(count) > limits.source_bytes as u128 {
                    return Err(SelectionError::LimitExceeded("source bytes"));
                }
                target.length = Some(count);
                if let Some(token) = token {
                    let PathToken::Index(index) = token else {
                        return Err(SelectionError::InvalidPath);
                    };
                    if *index >= count {
                        return Err(SelectionError::OutOfBounds);
                    }
                    if step + 1 != path.len() {
                        return Err(SelectionError::InvalidPath);
                    }
                    target.kind =
                        Kind::ProgressiveByte { position: *index as usize, length: count as usize };
                }
            }
            Kind::ByteList => {
                let bytes = packed(read(join(target.index, 3)?)?, 8)?;
                let count = u64::from_le_bytes(
                    bytes.as_ref().try_into().map_err(|_| SelectionError::InvalidValue)?,
                );
                if count > 32 {
                    return Err(SelectionError::InvalidContext);
                }
                target.length = Some(count);
                if let Some(token) = token {
                    let PathToken::Index(index) = token else {
                        return Err(SelectionError::InvalidPath);
                    };
                    if *index >= count {
                        return Err(SelectionError::OutOfBounds);
                    }
                    if step + 1 != path.len() {
                        return Err(SelectionError::InvalidPath);
                    }
                    target = Target {
                        index: join(target.index, 2)?,
                        kind: Kind::Byte { position: *index as usize, width: count as usize },
                        length: None,
                    };
                }
            }
            Kind::Bytes(width) if token.is_some() => {
                let Some(PathToken::Index(index)) = token else {
                    return Err(SelectionError::InvalidPath);
                };
                if *index >= width as u64 {
                    return Err(SelectionError::OutOfBounds);
                }
                if step + 1 != path.len() {
                    return Err(SelectionError::InvalidPath);
                }
                target = Target {
                    index: target.index,
                    kind: Kind::Byte { position: *index as usize, width },
                    length: None,
                };
            }
            Kind::Scalar(_) |
            Kind::Byte { .. } |
            Kind::ProgressiveByte { .. } |
            Kind::Inactive |
            Kind::Union { .. }
                if token.is_some() =>
            {
                return Err(SelectionError::InvalidPath)
            }
            Kind::Scalar(_) |
            Kind::Bytes(_) |
            Kind::Byte { .. } |
            Kind::ProgressiveByte { .. } |
            Kind::Inactive |
            Kind::Union { .. } => {}
        }
    }
    Ok(target)
}

fn segments(record: Record, bytes: &[u8]) -> Result<Vec<(Kind, &[u8])>, SelectionError> {
    let mut cursor = 0;
    let mut result = Vec::new();
    let mut offsets = Vec::new();
    for (_, kind) in record.fields() {
        let width = match kind {
            Kind::Inactive => 0,
            Kind::Scalar(width) | Kind::Bytes(width) => width,
            Kind::Record(record) if !record.variable() => record.width(),
            _ => 4,
        };
        let part = bytes.get(cursor..cursor + width).ok_or(SelectionError::InvalidValue)?;
        if matches!(
            kind,
            Kind::ByteList | Kind::List(..) | Kind::Union { .. } | Kind::ProgressiveBytes
        ) || matches!(kind, Kind::Record(record) if record.variable())
        {
            offsets.push((
                result.len(),
                u32::from_le_bytes(part.try_into().map_err(|_| SelectionError::InvalidValue)?)
                    as usize,
            ));
            result.push((kind, &bytes[0..0]));
        } else {
            result.push((kind, part));
        }
        cursor += width;
    }
    if offsets.is_empty() && cursor != bytes.len() {
        return Err(SelectionError::InvalidValue);
    }
    if !offsets.is_empty() && offsets[0].1 != cursor {
        return Err(SelectionError::InvalidValue);
    }
    for (index, (field, start)) in offsets.iter().enumerate() {
        let end = offsets.get(index + 1).map_or(bytes.len(), |(_, offset)| *offset);
        result[*field].1 = bytes.get(*start..end).ok_or(SelectionError::InvalidValue)?;
    }
    Ok(result)
}

fn supplied<'a>(
    snapshot: ComponentSnapshot<'a>,
    path: &[PathToken],
) -> Result<&'a [u8], SelectionError> {
    let mut bytes = snapshot.serialized();
    let mut kind = snapshot.object().kind();
    for token in path {
        if let Kind::Union { authorization } = kind {
            let (&selector, payload) = bytes.split_first().ok_or(SelectionError::InvalidValue)?;
            kind = Kind::Record(union_record(authorization, selector)?);
            bytes = payload;
        }
        match (kind, token) {
            (Kind::Record(record), PathToken::Field(name)) => {
                let index = record
                    .fields()
                    .iter()
                    .position(|(field, _)| *field == name)
                    .ok_or(SelectionError::InvalidPath)?;
                (kind, bytes) = segments(record, bytes)?[index];
            }
            (Kind::List(record, _), PathToken::Index(index)) => {
                bytes = list_item(
                    record,
                    bytes,
                    usize::try_from(*index).map_err(|_| SelectionError::OutOfBounds)?,
                )?;
                kind = if record == Record::Hash { Kind::Bytes(32) } else { Kind::Record(record) };
            }
            _ => return Err(SelectionError::InvalidPath),
        }
    }
    Ok(bytes)
}

fn root_of(
    kind: Kind,
    bytes: &[u8],
    work: &mut usize,
    limit: usize,
) -> Result<B256, SelectionError> {
    add(work, bytes.len(), limit, "source bytes")?;
    let node = match kind {
        Kind::Inactive => {
            if !bytes.is_empty() {
                return Err(SelectionError::InvalidValue);
            }
            Ok(RetainedNode::zero())
        }
        Kind::Union { authorization } => {
            let (&selector, payload) = bytes.split_first().ok_or(SelectionError::InvalidValue)?;
            let record = union_record(authorization, selector)?;
            let root = root_of(Kind::Record(record), payload, work, limit)?;
            add(work, 3 * size_of::<RetainedNode>(), limit, "source bytes")?;
            Ok(RetainedNode::pair(
                RetainedNode::leaf(root),
                RetainedNode::leaf(B256::right_padding_from(&[selector])),
            ))
        }
        Kind::ProgressiveBytes => {
            let nodes = bytes
                .len()
                .div_ceil(32)
                .checked_add(1)
                .and_then(|nodes| nodes.checked_mul(8 * size_of::<RetainedNode>()))
                .ok_or(SelectionError::LimitExceeded("source bytes"))?;
            add(work, nodes, limit, "source bytes")?;
            reth_pureth_ssz::progressive_byte_list(bytes)
        }
        Kind::Scalar(width) | Kind::Bytes(width) => {
            if bytes.len() != width {
                return Err(SelectionError::InvalidValue);
            }
            let count = width.div_ceil(32).next_power_of_two();
            add(work, count * 2 * size_of::<RetainedNode>(), limit, "source bytes")?;
            merkleize_fixed(
                bytes
                    .chunks(32)
                    .map(|bytes| RetainedNode::leaf(B256::right_padding_from(bytes)))
                    .collect(),
                count,
            )
        }
        Kind::Byte { .. } | Kind::ProgressiveByte { .. } => {
            return Err(SelectionError::UnsupportedOperation)
        }
        Kind::ByteList => {
            if bytes.len() > 32 {
                return Err(SelectionError::InvalidValue);
            }
            add(work, size_of::<RetainedNode>() * 3, limit, "source bytes")?;
            Ok(mix_in_length(RetainedNode::leaf(B256::right_padding_from(bytes)), bytes.len()))
        }
        Kind::Record(record) => {
            if record == Record::Summary {
                reth_pureth_block::ExecutionSummary::from_ssz_bytes(bytes)
                    .map_err(|_| SelectionError::InvalidValue)?;
            }
            let fields = segments(record, bytes)?;
            add(work, fields.len() * 8 * size_of::<RetainedNode>(), limit, "source bytes")?;
            let nodes = fields
                .into_iter()
                .map(|(kind, bytes)| root_of(kind, bytes, work, limit).map(RetainedNode::leaf))
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(mask) = record.mask() {
                merkleize_progressive(nodes).map(|tree| {
                    RetainedNode::pair(
                        tree,
                        RetainedNode::leaf(B256::right_padding_from(&mask.to_le_bytes())),
                    )
                })
            } else {
                merkleize_fixed(nodes, record.fields().len().next_power_of_two())
            }
        }
        Kind::List(record, bound) => {
            let count = list_count(record, bytes)?;
            if bound != 0 && count > bound {
                return Err(SelectionError::InvalidValue);
            }
            let nodes_bound = if bound == 0 {
                count.checked_mul(8).ok_or(SelectionError::LimitExceeded("source bytes"))?
            } else {
                bound * 2
            };
            add(
                work,
                nodes_bound
                    .checked_mul(size_of::<RetainedNode>())
                    .ok_or(SelectionError::LimitExceeded("source bytes"))?,
                limit,
                "source bytes",
            )?;
            let nodes = (0..count)
                .map(|index| {
                    root_of(Kind::Record(record), list_item(record, bytes, index)?, work, limit)
                        .map(RetainedNode::leaf)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let contents = if bound == 0 {
                merkleize_progressive(nodes)
            } else {
                merkleize_fixed(nodes, bound)
            }
            .map_err(|_| SelectionError::InvalidValue)?;
            Ok(mix_in_length(contents, count))
        }
    }
    .map_err(|_| SelectionError::InvalidValue)?;
    Ok(node.root())
}

fn whole_work(
    kind: Kind,
    bytes: &[u8],
    work: &mut usize,
    limit: usize,
    cancelled: &AtomicBool,
) -> Result<(), SelectionError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(SelectionError::Cancelled);
    }
    add(work, bytes.len(), limit, "source bytes")?;
    match kind {
        Kind::Inactive => {
            if bytes.is_empty() {
                Ok(())
            } else {
                Err(SelectionError::InvalidValue)
            }
        }
        Kind::Union { authorization } => {
            let (&selector, payload) = bytes.split_first().ok_or(SelectionError::InvalidValue)?;
            add(work, 3 * size_of::<RetainedNode>(), limit, "source bytes")?;
            whole_work(
                Kind::Record(union_record(authorization, selector)?),
                payload,
                work,
                limit,
                cancelled,
            )
        }
        Kind::ProgressiveBytes => add(
            work,
            bytes
                .len()
                .div_ceil(32)
                .checked_add(1)
                .and_then(|nodes| nodes.checked_mul(8 * size_of::<RetainedNode>()))
                .ok_or(SelectionError::LimitExceeded("source bytes"))?,
            limit,
            "source bytes",
        ),
        Kind::Scalar(width) | Kind::Bytes(width) => add(
            work,
            width.div_ceil(32).next_power_of_two() * 2 * size_of::<RetainedNode>(),
            limit,
            "source bytes",
        ),
        Kind::ByteList => add(work, 3 * size_of::<RetainedNode>(), limit, "source bytes"),
        Kind::Record(record) => {
            let fields = segments(record, bytes)?;
            add(work, fields.len() * 8 * size_of::<RetainedNode>(), limit, "source bytes")?;
            for (kind, bytes) in fields {
                whole_work(kind, bytes, work, limit, cancelled)?;
            }
            Ok(())
        }
        Kind::List(record, bound) => {
            let count = list_count(record, bytes)?;
            if bound != 0 && count > bound {
                return Err(SelectionError::InvalidValue);
            }
            let nodes = if bound == 0 {
                count.checked_mul(8).ok_or(SelectionError::LimitExceeded("source bytes"))?
            } else {
                bound * 2
            };
            add(
                work,
                nodes
                    .checked_mul(size_of::<RetainedNode>())
                    .ok_or(SelectionError::LimitExceeded("source bytes"))?,
                limit,
                "source bytes",
            )?;
            for index in 0..count {
                whole_work(
                    Kind::Record(record),
                    list_item(record, bytes, index)?,
                    work,
                    limit,
                    cancelled,
                )?;
            }
            Ok(())
        }
        Kind::Byte { .. } | Kind::ProgressiveByte { .. } => {
            Err(SelectionError::UnsupportedOperation)
        }
    }
}

fn value(
    target: Target,
    read: &mut impl FnMut(u128) -> Result<B256, SelectionError>,
) -> Result<Bytes, SelectionError> {
    match target.kind {
        Kind::Inactive => Err(SelectionError::AbsentField),
        Kind::ProgressiveByte { position, length } => {
            progressive_slice(target.index, length, position, position + 1, read)
        }
        Kind::ProgressiveBytes => progressive_slice(
            target.index,
            target.length.ok_or(SelectionError::InvalidContext)? as usize,
            0,
            target.length.ok_or(SelectionError::InvalidContext)? as usize,
            read,
        ),
        Kind::Byte { position, width } => {
            let packed_target =
                Target { index: target.index, kind: Kind::Bytes(width), length: None };
            Ok(Bytes::from(vec![value(packed_target, read)?[position]]))
        }
        Kind::Scalar(width) => packed(read(target.index)?, width),
        Kind::Bytes(width) if width <= 32 => packed(read(target.index)?, width),
        Kind::Bytes(width) => {
            let mut bytes = Vec::with_capacity(width);
            let count = width.div_ceil(32).next_power_of_two();
            for chunk in 0..width.div_ceil(32) {
                bytes.extend_from_slice(&packed(
                    read(join(target.index, (count + chunk) as u128)?)?,
                    (width - chunk * 32).min(32),
                )?);
            }
            for chunk in width.div_ceil(32)..count {
                if read(join(target.index, (count + chunk) as u128)?)? != B256::ZERO {
                    return Err(SelectionError::InvalidValue);
                }
            }
            Ok(Bytes::from(bytes))
        }
        Kind::ByteList => packed(
            read(join(target.index, 2)?)?,
            target.length.ok_or(SelectionError::InvalidContext)? as usize,
        ),
        _ => Err(SelectionError::CodecRequired),
    }
}

fn progressive_slice(
    index: u128,
    length: usize,
    start: usize,
    end: usize,
    read: &mut impl FnMut(u128) -> Result<B256, SelectionError>,
) -> Result<Bytes, SelectionError> {
    if start > end || end > length {
        return Err(SelectionError::OutOfBounds);
    }
    let mut bytes = Vec::with_capacity(end - start);
    for chunk in start / 32..end.div_ceil(32) {
        let node = read(join(
            index,
            progressive_chunk_gindex(chunk as u64)
                .map_err(|_| SelectionError::LimitExceeded("proof depth"))?,
        )?)?;
        let width = (length - chunk * 32).min(32);
        let data = packed(node, width)?;
        let left = start.saturating_sub(chunk * 32);
        let right = (end - chunk * 32).min(width);
        bytes.extend_from_slice(&data[left..right]);
    }
    Ok(Bytes::from(bytes))
}

fn evaluate(
    object: ComponentObject,
    selection: &ReceiptSelection,
    read: &mut impl FnMut(u128) -> Result<B256, SelectionError>,
    whole: &mut impl FnMut(&[PathToken], Kind) -> Result<Bytes, SelectionError>,
    targets: &mut usize,
    mut work: Option<&mut usize>,
    limits: SelectionLimits,
) -> Result<Vec<Bytes>, SelectionError> {
    let path = tokens(&selection.path)?;
    let target = resolve(object, &path, read, limits)?;
    let mut one = |path: &[PathToken], target: Target| {
        add(targets, 1, limits.targets, "targets")?;
        if matches!(target.kind, Kind::Record(_) | Kind::List(..) | Kind::Union { .. }) {
            let bytes = whole(path, target.kind)?;
            let node = read(target.index)?;
            if let Some(work) = work.as_deref_mut() &&
                root_of(target.kind, &bytes, work, limits.source_bytes)? != node
            {
                return Err(SelectionError::InvalidValue);
            }
            Ok(bytes)
        } else {
            if matches!(target.kind, Kind::ProgressiveBytes) &&
                target
                    .length
                    .is_some_and(|length| u128::from(length) > limits.response_bytes as u128)
            {
                return Err(SelectionError::LimitExceeded("value bytes"));
            }
            value(target, read)
        }
    };
    match selection.operation {
        SelectionOperation::Whole {} | SelectionOperation::Value {} => {
            Ok(vec![one(&path, target)?])
        }
        SelectionOperation::Length {} => {
            add(targets, 1, limits.targets, "targets")?;
            let length = match target.kind {
                Kind::Bytes(width) => width as u64,
                _ => target.length.ok_or(SelectionError::UnsupportedOperation)?,
            };
            Ok(vec![Bytes::from(length.to_le_bytes().to_vec())])
        }
        SelectionOperation::Variant {} if matches!(target.kind, Kind::Union { .. }) => {
            add(targets, 1, limits.targets, "targets")?;
            Ok(vec![packed(read(join(target.index, 3)?)?, 1)?])
        }
        SelectionOperation::Presence {} if matches!(path.last(), Some(PathToken::Field(_))) => {
            add(targets, 1, limits.targets, "targets")?;
            let present = !matches!(target.kind, Kind::Inactive);
            if !present && read(target.index)? != B256::ZERO {
                return Err(SelectionError::InvalidContext);
            }
            Ok(vec![Bytes::from(vec![u8::from(present)])])
        }
        SelectionOperation::Range { start, end } | SelectionOperation::Slice { start, end } => {
            if matches!(selection.operation, SelectionOperation::Slice { .. }) &&
                !matches!(target.kind, Kind::ByteList | Kind::Bytes(_) | Kind::ProgressiveBytes)
            {
                return Err(SelectionError::UnsupportedOperation);
            }
            let length = match target.kind {
                Kind::Bytes(width) => width as u64,
                _ => target.length.ok_or(SelectionError::UnsupportedOperation)?,
            };
            if start > end || end > length {
                return Err(SelectionError::OutOfBounds);
            }
            let count = usize::try_from(end - start)
                .map_err(|_| SelectionError::LimitExceeded("targets"))?;
            if count > limits.targets.saturating_sub(*targets) {
                return Err(SelectionError::LimitExceeded("targets"));
            }
            add(targets, count.max(1), limits.targets, "targets")?;
            let mut selected = Vec::new();
            match target.kind {
                Kind::List(record, _) => {
                    for index in start..end {
                        let mut child_path = path.clone();
                        child_path.push(PathToken::Index(index));
                        let child = resolve(object, &child_path, read, limits)?;
                        let bytes = whole(&child_path, Kind::Record(record))?;
                        let node = read(child.index)?;
                        if let Some(work) = work.as_deref_mut() &&
                            root_of(child.kind, &bytes, work, limits.source_bytes)? != node
                        {
                            return Err(SelectionError::InvalidValue);
                        }
                        selected.push(bytes);
                    }
                }
                Kind::ProgressiveBytes => {
                    let bytes = progressive_slice(
                        target.index,
                        length as usize,
                        start as usize,
                        end as usize,
                        read,
                    )?;
                    if matches!(selection.operation, SelectionOperation::Slice { .. }) {
                        selected.push(bytes);
                    } else {
                        selected.extend(bytes.iter().map(|byte| Bytes::from(vec![*byte])));
                    }
                }
                Kind::ByteList | Kind::Bytes(_) => {
                    let bytes = value(target, read)?;
                    if matches!(selection.operation, SelectionOperation::Slice { .. }) {
                        selected.push(Bytes::copy_from_slice(&bytes[start as usize..end as usize]));
                    } else {
                        selected.extend(
                            bytes[start as usize..end as usize]
                                .iter()
                                .map(|byte| Bytes::from(vec![*byte])),
                        );
                    }
                }
                _ => return Err(SelectionError::UnsupportedOperation),
            }
            Ok(selected)
        }
        _ => Err(SelectionError::UnsupportedOperation),
    }
}

pub fn select_component(
    snapshot: ComponentSnapshot<'_>,
    request: &SelectionRequest,
    limits: SelectionLimits,
) -> Result<ComponentResponse, SelectionError> {
    select_component_with_cancel(snapshot, request, limits, &AtomicBool::new(false))
}

pub fn select_component_with_cancel(
    snapshot: ComponentSnapshot<'_>,
    request: &SelectionRequest,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<ComponentResponse, SelectionError> {
    check_request(request, limits)?;
    let mut source = snapshot
        .serialized()
        .len()
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(256))
        .ok_or(SelectionError::LimitExceeded("source bytes"))?;
    if source > limits.source_bytes {
        return Err(SelectionError::LimitExceeded("source bytes"));
    }
    let mut pending = vec![(snapshot.tree(), 0)];
    while let Some((node, depth)) = pending.pop() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(SelectionError::Cancelled);
        }
        if depth > 127 {
            return Err(SelectionError::LimitExceeded("proof depth"));
        }
        add(&mut source, size_of::<RetainedNode>(), limits.source_bytes, "source bytes")?;
        if let Some(children) = node.children() {
            pending.extend(children.iter().map(|child| (child, depth + 1)));
        }
    }
    let mut reads = BTreeMap::new();
    let mut read_order = Vec::new();
    let proof_work = Cell::new(0usize);
    let check_materialization = |values, work: usize| {
        let available = remaining_materialization(source, values, 0, limits.source_bytes)?;
        let required = proof_work
            .get()
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(work))
            .ok_or(SelectionError::LimitExceeded("source bytes"))?;
        if required > available {
            return Err(SelectionError::LimitExceeded("source bytes"));
        }
        Ok(())
    };
    let mut targets = 0;
    let mut value_bytes = 0;
    let mut verification_work = 0;
    let mut results = Vec::new();
    for selection in &request.selections {
        let previous_value_bytes = value_bytes;
        let mut read = |index| {
            if cancelled.load(Ordering::Relaxed) {
                return Err(SelectionError::Cancelled);
            }
            let node = node_at(snapshot.tree(), index).map_err(|_| SelectionError::InvalidProof)?;
            if request.include_proof && reads.insert(index, node).is_none() {
                read_order.push(index);
            }
            if reads.len() > limits.witness_hashes {
                return Err(SelectionError::LimitExceeded("witness hashes"));
            }
            proof_work.set(
                reads
                    .len()
                    .checked_mul(multiproof::PROOF_NODE_BYTES)
                    .ok_or(SelectionError::LimitExceeded("source bytes"))?,
            );
            if proof_work.get().checked_add(source).is_none_or(|bytes| bytes > limits.source_bytes)
            {
                return Err(SelectionError::LimitExceeded("source bytes"));
            }
            Ok(node)
        };
        if matches!(
            selection.operation,
            SelectionOperation::Whole {} |
                SelectionOperation::Value {} |
                SelectionOperation::Range { .. } |
                SelectionOperation::Slice { .. }
        ) {
            let target = resolve(snapshot.object(), &tokens(&selection.path)?, &mut read, limits)?;
            if matches!(target.kind, Kind::ProgressiveBytes) {
                let length = target.length.ok_or(SelectionError::InvalidContext)?;
                let length = match selection.operation {
                    SelectionOperation::Range { start, end } |
                    SelectionOperation::Slice { start, end } => {
                        if start > end || end > length {
                            return Err(SelectionError::OutOfBounds);
                        }
                        end - start
                    }
                    _ => length,
                };
                let length = usize::try_from(length)
                    .map_err(|_| SelectionError::LimitExceeded("source bytes"))?;
                let mut reserved = previous_value_bytes;
                add(&mut reserved, length, limits.response_bytes, "value bytes")?;
                check_materialization(reserved, verification_work)?;
            }
        }
        let mut whole = |path: &[PathToken], kind| {
            let bytes = supplied(snapshot, path)?;
            add(&mut value_bytes, bytes.len(), limits.response_bytes, "value bytes")?;
            whole_work(kind, bytes, &mut verification_work, limits.source_bytes, cancelled)?;
            check_materialization(value_bytes, verification_work)?;
            Ok(Bytes::copy_from_slice(bytes))
        };
        let values = evaluate(
            snapshot.object(),
            selection,
            &mut read,
            &mut whole,
            &mut targets,
            None,
            limits,
        )?;
        let total = values.iter().try_fold(0usize, |total, value| {
            total.checked_add(value.len()).ok_or(SelectionError::LimitExceeded("value bytes"))
        })?;
        value_bytes = previous_value_bytes;
        add(&mut value_bytes, total, limits.response_bytes, "value bytes")?;
        check_materialization(value_bytes, verification_work)?;
        results.push(SelectionResult { values_ssz: values, witnesses: None, matches: None });
    }
    let available = limits
        .source_bytes
        .checked_sub(source)
        .and_then(|bytes| bytes.checked_sub(value_bytes.checked_mul(2)?))
        .and_then(|bytes| bytes.checked_sub(verification_work))
        .ok_or(SelectionError::LimitExceeded("source bytes"))?;
    let multiproof = if request.include_proof {
        Some(multiproof::build(
            snapshot.tree(),
            &reads,
            &read_order,
            limits,
            available / 2,
            cancelled,
        )?)
    } else {
        None
    };
    let response = ComponentResponse {
        object: snapshot.object(),
        selection: SelectionResponse {
            root: snapshot.tree().root(),
            proof_format: request.include_proof.then_some(SelectionProofFormat::Multiproof),
            results,
            multiproof,
        },
    };
    encoded_size(&response, limits.response_bytes, "response bytes")?;
    Ok(response)
}

pub fn verify_component_selection(
    object: ComponentObject,
    request: &SelectionRequest,
    response: &ComponentResponse,
    expected_root: B256,
    limits: SelectionLimits,
) -> Result<(), SelectionError> {
    check_request(request, limits)?;
    if !request.include_proof {
        return Err(SelectionError::Unverified);
    }
    if response.object != object ||
        response.selection.root != expected_root ||
        response.selection.results.len() != request.selections.len() ||
        response.selection.proof_format != Some(SelectionProofFormat::Multiproof)
    {
        return Err(SelectionError::InvalidContext);
    }
    encoded_size(response, limits.response_bytes, "response bytes")?;
    let proof = response.selection.multiproof.as_ref().ok_or(SelectionError::InvalidProof)?;
    let mut reads = multiproof::ProofReads::new(proof, limits)?;
    let mut targets = 0;
    let mut work = multiproof::proof_work(proof, limits)?;
    for result in &response.selection.results {
        for value in &result.values_ssz {
            add(
                &mut work,
                value.len().checked_mul(2).ok_or(SelectionError::LimitExceeded("source bytes"))?,
                limits.source_bytes,
                "source bytes",
            )?;
        }
    }
    for (selection, result) in request.selections.iter().zip(&response.selection.results) {
        if result.witnesses.is_some() || result.matches.is_some() {
            return Err(SelectionError::InvalidProof);
        }
        let mut supplied = result.values_ssz.iter();
        let mut read = |index| reads.read(index);
        let values = evaluate(
            object,
            selection,
            &mut read,
            &mut |_, _| supplied.next().cloned().ok_or(SelectionError::InvalidValue),
            &mut targets,
            Some(&mut work),
            limits,
        )?;
        if values != result.values_ssz {
            return Err(SelectionError::InvalidValue);
        }
    }
    reads.finish(proof, expected_root, limits)
}

#[cfg(test)]
mod tests;
