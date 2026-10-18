use super::{
    add, claim_value, collection, length, resolve_eip, Kind, SelectionError, SelectionLimits,
};
use crate::PathToken;
use alloy_primitives::{Address, B256};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOutput {
    Receipts,
    Logs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FilterPredicate {
    Sender { address: Address },
    Status { value: bool },
    GasUsed { min: u64, max: u64 },
    LogAddress { address: Address },
    LogTopic { position: u8, topic: B256 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterMatch {
    pub receipt_index: u64,
    pub log_index: Option<u64>,
}

impl FilterMatch {
    pub(super) fn path(&self) -> Vec<PathToken> {
        let mut path = vec![PathToken::Index(self.receipt_index)];
        if let Some(index) = self.log_index {
            path.extend([PathToken::Field("logs".into()), PathToken::Index(index)]);
        }
        path
    }
}

pub(super) fn check_predicates(predicates: &[FilterPredicate]) -> Result<(), SelectionError> {
    for predicate in predicates {
        match predicate {
            FilterPredicate::GasUsed { min, max } if min > max => {
                return Err(SelectionError::OutOfBounds);
            }
            FilterPredicate::LogTopic { position, .. } if *position > 3 => {
                return Err(SelectionError::OutOfBounds);
            }
            _ => {}
        }
    }
    Ok(())
}

const fn is_log(predicate: &FilterPredicate) -> bool {
    matches!(predicate, FilterPredicate::LogAddress { .. } | FilterPredicate::LogTopic { .. })
}

fn test(
    predicate: &FilterPredicate,
    path: &mut Vec<PathToken>,
    read: &mut impl FnMut(u128) -> Result<B256, SelectionError>,
    targets: &mut usize,
    limits: SelectionLimits,
) -> Result<bool, SelectionError> {
    add(targets, 1, limits.targets, "targets")?;
    let field = match predicate {
        FilterPredicate::Sender { .. } => "from",
        FilterPredicate::Status { .. } => "status",
        FilterPredicate::GasUsed { .. } => "gas_used",
        FilterPredicate::LogAddress { .. } => "address",
        FilterPredicate::LogTopic { .. } => "topics",
    };
    path.push(PathToken::Field(field.into()));
    if let FilterPredicate::LogTopic { position, .. } = predicate {
        let list = resolve_eip(path, read)?;
        if u64::from(*position) >= list.length.ok_or(SelectionError::InvalidContext)? {
            path.pop();
            return Ok(false);
        }
        path.push(PathToken::Index(u64::from(*position)));
    }
    let target = resolve_eip(path, read)?;
    let value = claim_value(target, read)?;
    let matched = match predicate {
        FilterPredicate::Sender { address } | FilterPredicate::LogAddress { address } => {
            value.as_ref() == address.as_slice()
        }
        FilterPredicate::Status { value: expected } => {
            if value[0] > 1 {
                return Err(SelectionError::InvalidValue);
            }
            (value[0] == 1) == *expected
        }
        FilterPredicate::GasUsed { min, max } => {
            let gas = length(read(target.index)?)?;
            (*min..=*max).contains(&gas)
        }
        FilterPredicate::LogTopic { topic, .. } => value.as_ref() == topic.as_slice(),
    };
    if matches!(predicate, FilterPredicate::LogTopic { .. }) {
        path.pop();
    }
    path.pop();
    Ok(matched)
}

pub(super) fn scan(
    output: FilterOutput,
    predicates: &[FilterPredicate],
    read: &mut impl FnMut(u128) -> Result<B256, SelectionError>,
    targets: &mut usize,
    matches: &mut usize,
    limits: SelectionLimits,
) -> Result<Vec<FilterMatch>, SelectionError> {
    check_predicates(predicates)?;
    let root = collection(1, Kind::Receipts, read)?;
    let count = usize::try_from(root.length.ok_or(SelectionError::InvalidContext)?)
        .map_err(|_| SelectionError::LimitExceeded("receipts"))?;
    if count > limits.receipts {
        return Err(SelectionError::LimitExceeded("receipts"));
    }
    add(targets, 1, limits.targets, "targets")?;
    let mut result = Vec::new();
    let mut logs = 0;
    let has_log = predicates.iter().any(is_log);
    for receipt_index in 0..count as u64 {
        let mut path = vec![PathToken::Index(receipt_index)];
        let mut log_path = path.clone();
        log_path.push(PathToken::Field("logs".into()));
        add(targets, 1, limits.targets, "targets")?;
        let list = resolve_eip(&log_path, read)?;
        let log_count = usize::try_from(list.length.ok_or(SelectionError::InvalidContext)?)
            .map_err(|_| SelectionError::LimitExceeded("logs"))?;
        add(&mut logs, log_count, limits.logs, "logs")?;
        let mut receipt_matches = true;
        for predicate in predicates.iter().filter(|predicate| !is_log(predicate)) {
            if !test(predicate, &mut path, read, targets, limits)? {
                receipt_matches = false;
                break;
            }
        }
        if !receipt_matches {
            continue;
        }
        if output == FilterOutput::Receipts && !has_log {
            add(matches, 1, limits.matches, "matches")?;
            add(targets, 1, limits.targets, "targets")?;
            result.push(FilterMatch { receipt_index, log_index: None });
            continue;
        }
        for log_index in 0..log_count as u64 {
            let mut path = log_path.clone();
            path.push(PathToken::Index(log_index));
            add(targets, 1, limits.targets, "targets")?;
            let mut log_matches = true;
            for predicate in predicates.iter().filter(|predicate| is_log(predicate)) {
                if !test(predicate, &mut path, read, targets, limits)? {
                    log_matches = false;
                    break;
                }
            }
            if log_matches {
                add(matches, 1, limits.matches, "matches")?;
                result.push(FilterMatch {
                    receipt_index,
                    log_index: (output == FilterOutput::Logs).then_some(log_index),
                });
                if output == FilterOutput::Receipts {
                    break;
                }
            }
        }
    }
    Ok(result)
}
