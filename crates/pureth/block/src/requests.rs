use crate::uint64;
use alloy_primitives::{Address, Bytes, FixedBytes, B256};
use reth_pureth_ssz::{merkleize_fixed, mix_in_length, RetainedNode, TreeConstructionError};
use ssz::{Decode, DecodeError, SszDecoderBuilder, SszEncoder};
use ssz_derive::{Decode, Encode};
use std::fmt;

#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct DepositRequest {
    pub pubkey: FixedBytes<48>,
    pub withdrawal_credentials: B256,
    pub amount: u64,
    pub signature: FixedBytes<96>,
    pub index: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct WithdrawalRequest {
    pub source_address: Address,
    pub validator_pubkey: FixedBytes<48>,
    pub amount: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct ConsolidationRequest {
    pub source_address: Address,
    pub source_pubkey: FixedBytes<48>,
    pub target_pubkey: FixedBytes<48>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecutionRequests {
    pub deposits: Vec<DepositRequest>,
    pub withdrawals: Vec<WithdrawalRequest>,
    pub consolidations: Vec<ConsolidationRequest>,
}

#[derive(Debug)]
pub enum RequestsError {
    LimitExceeded(&'static str),
    InvalidGroups(&'static str),
    UnsupportedType(u8),
    Decode(DecodeError),
    Tree(TreeConstructionError),
}

impl fmt::Display for RequestsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitExceeded(name) => write!(f, "execution requests exceed {name} limit"),
            Self::InvalidGroups(reason) => write!(f, "invalid execution request groups: {reason}"),
            Self::UnsupportedType(tag) => write!(f, "unsupported execution request type: {tag}"),
            Self::Decode(error) => write!(f, "execution requests decoding failed: {error:?}"),
            Self::Tree(error) => write!(f, "execution requests tree failed: {error}"),
        }
    }
}

impl std::error::Error for RequestsError {}

impl From<TreeConstructionError> for RequestsError {
    fn from(error: TreeConstructionError) -> Self {
        Self::Tree(error)
    }
}

impl ExecutionRequests {
    pub fn from_native_groups(groups: &[Bytes]) -> Result<Self, RequestsError> {
        let mut requests = Self::default();
        let mut previous = None;
        for group in groups {
            let (&tag, body) =
                group.split_first().ok_or(RequestsError::InvalidGroups("empty group"))?;
            if tag > 2 {
                return Err(RequestsError::UnsupportedType(tag));
            }
            if previous.is_some_and(|previous| tag <= previous) {
                return Err(RequestsError::InvalidGroups("unordered or duplicate type"));
            }
            if body.is_empty() {
                return Err(RequestsError::InvalidGroups("empty request body"));
            }
            match tag {
                0 => {
                    requests.deposits =
                        decode_list(body, 192, 8192).map_err(RequestsError::Decode)?
                }
                1 => {
                    requests.withdrawals =
                        decode_list(body, 76, 16).map_err(RequestsError::Decode)?
                }
                2 => {
                    requests.consolidations =
                        decode_list(body, 116, 2).map_err(RequestsError::Decode)?
                }
                _ => unreachable!(),
            }
            previous = Some(tag);
        }
        Ok(requests)
    }

    pub fn to_ssz_bytes(&self) -> Result<Vec<u8>, RequestsError> {
        self.check_limits()?;
        let mut bytes = Vec::new();
        let mut encoder = SszEncoder::container(&mut bytes, 12);
        encoder.append(&self.deposits);
        encoder.append(&self.withdrawals);
        encoder.append(&self.consolidations);
        encoder.finalize();
        Ok(bytes)
    }

    pub fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, RequestsError> {
        let mut builder = SszDecoderBuilder::new(bytes);
        for _ in 0..3 {
            builder.register_type::<Vec<u8>>().map_err(RequestsError::Decode)?;
        }
        let mut decoder = builder.build().map_err(RequestsError::Decode)?;
        let requests = Self {
            deposits: decoder
                .decode_next_with(|bytes| decode_list::<DepositRequest>(bytes, 192, 8192))
                .map_err(RequestsError::Decode)?,
            withdrawals: decoder
                .decode_next_with(|bytes| decode_list::<WithdrawalRequest>(bytes, 76, 16))
                .map_err(RequestsError::Decode)?,
            consolidations: decoder
                .decode_next_with(|bytes| decode_list::<ConsolidationRequest>(bytes, 116, 2))
                .map_err(RequestsError::Decode)?,
        };
        requests.check_limits()?;
        Ok(requests)
    }

    fn check_limits(&self) -> Result<(), RequestsError> {
        for (length, bound, name) in [
            (self.deposits.len(), 8192, "deposits"),
            (self.withdrawals.len(), 16, "withdrawals"),
            (self.consolidations.len(), 2, "consolidations"),
        ] {
            if length > bound {
                return Err(RequestsError::LimitExceeded(name));
            }
        }
        Ok(())
    }

    pub fn tree(&self) -> Result<RetainedNode, RequestsError> {
        self.check_limits()?;
        let deposits = self
            .deposits
            .iter()
            .map(|value| {
                merkleize_fixed(
                    vec![
                        bytes_tree(value.pubkey.as_slice())?,
                        RetainedNode::leaf(value.withdrawal_credentials),
                        uint64(value.amount),
                        bytes_tree(value.signature.as_slice())?,
                        uint64(value.index),
                    ],
                    8,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let withdrawals = self
            .withdrawals
            .iter()
            .map(|value| {
                merkleize_fixed(
                    vec![
                        bytes_tree(value.source_address.as_slice())?,
                        bytes_tree(value.validator_pubkey.as_slice())?,
                        uint64(value.amount),
                    ],
                    4,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let consolidations = self
            .consolidations
            .iter()
            .map(|value| {
                merkleize_fixed(
                    vec![
                        bytes_tree(value.source_address.as_slice())?,
                        bytes_tree(value.source_pubkey.as_slice())?,
                        bytes_tree(value.target_pubkey.as_slice())?,
                    ],
                    4,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(merkleize_fixed(
            vec![
                mix_in_length(merkleize_fixed(deposits, 8192)?, self.deposits.len()),
                mix_in_length(merkleize_fixed(withdrawals, 16)?, self.withdrawals.len()),
                mix_in_length(merkleize_fixed(consolidations, 2)?, self.consolidations.len()),
            ],
            4,
        )?)
    }
}

fn decode_list<T: Decode>(bytes: &[u8], width: usize, bound: usize) -> Result<Vec<T>, DecodeError> {
    if !bytes.len().is_multiple_of(width) || bytes.len() / width > bound {
        return Err(DecodeError::BytesInvalid("invalid execution request list length".into()));
    }
    Vec::<T>::from_ssz_bytes(bytes)
}

fn bytes_tree(bytes: &[u8]) -> Result<RetainedNode, TreeConstructionError> {
    let nodes =
        bytes.chunks(32).map(|bytes| RetainedNode::leaf(B256::right_padding_from(bytes))).collect();
    merkleize_fixed(nodes, bytes.len().div_ceil(32).next_power_of_two())
}

#[derive(Debug)]
pub struct ExecutionRequestsSnapshot {
    requests: ExecutionRequests,
    serialized: Bytes,
    tree: RetainedNode,
}

impl ExecutionRequestsSnapshot {
    pub fn from_native_groups(groups: &[Bytes]) -> Result<Self, RequestsError> {
        Self::build(ExecutionRequests::from_native_groups(groups)?)
    }
    pub fn build(requests: ExecutionRequests) -> Result<Self, RequestsError> {
        let serialized = Bytes::from(requests.to_ssz_bytes()?);
        let tree = requests.tree()?;
        Ok(Self { requests, serialized, tree })
    }
    pub fn from_ssz(bytes: &[u8]) -> Result<Self, RequestsError> {
        Self::build(ExecutionRequests::from_ssz_bytes(bytes)?)
    }
    pub const fn requests(&self) -> &ExecutionRequests {
        &self.requests
    }
    pub fn serialized(&self) -> &[u8] {
        &self.serialized
    }
    pub const fn tree(&self) -> &RetainedNode {
        &self.tree
    }
    pub const fn root(&self) -> B256 {
        self.tree.root()
    }
}
