use crate::path::PathToken;

pub const SCHEMA_ID: &str = "pureth-receipt-v0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedPath {
    ReceiptLogAddress { receipt_index: u64, log_index: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedPath;

pub fn resolve(tokens: &[PathToken]) -> Result<ResolvedPath, UnsupportedPath> {
    match tokens {
        [PathToken::Index(receipt_index), PathToken::Field(logs), PathToken::Index(log_index), PathToken::Field(address)]
            if logs == "logs" && address == "address" =>
        {
            Ok(ResolvedPath::ReceiptLogAddress {
                receipt_index: *receipt_index,
                log_index: *log_index,
            })
        }
        _ => Err(UnsupportedPath),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundsError {
    IndexTooLarge,
    ReceiptOutOfBounds,
    LogOutOfBounds,
}

pub fn validate_runtime_bounds(
    path: ResolvedPath,
    receipt_log_counts: &[usize],
) -> Result<(usize, usize), BoundsError> {
    let ResolvedPath::ReceiptLogAddress { receipt_index, log_index } = path;

    let receipt_index = usize::try_from(receipt_index).map_err(|_| BoundsError::IndexTooLarge)?;
    let log_index = usize::try_from(log_index).map_err(|_| BoundsError::IndexTooLarge)?;
    let log_count = receipt_log_counts.get(receipt_index).ok_or(BoundsError::ReceiptOutOfBounds)?;

    if log_index >= *log_count {
        return Err(BoundsError::LogOutOfBounds);
    }

    Ok((receipt_index, log_index))
}

pub(crate) fn validate_runtime_lengths(
    path: ResolvedPath,
    receipt_count: usize,
    log_count: usize,
) -> Result<(usize, usize), BoundsError> {
    let ResolvedPath::ReceiptLogAddress { receipt_index, log_index } = path;
    let receipt_index = usize::try_from(receipt_index).map_err(|_| BoundsError::IndexTooLarge)?;
    let log_index = usize::try_from(log_index).map_err(|_| BoundsError::IndexTooLarge)?;

    if receipt_index >= receipt_count {
        return Err(BoundsError::ReceiptOutOfBounds);
    }
    if log_index >= log_count {
        return Err(BoundsError::LogOutOfBounds);
    }

    Ok((receipt_index, log_index))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GindexError {
    ZeroGindex,
    InvalidContainerField,
    Overflow,
}

const fn append_bits(prefix: u64, bits: u64, width: u32) -> Result<u64, GindexError> {
    if width >= u64::BITS || prefix > (u64::MAX >> width) {
        return Err(GindexError::Overflow);
    }

    Ok((prefix << width) | bits)
}

pub const fn compose_gindices(parent: u64, child: u64) -> Result<u64, GindexError> {
    if parent == 0 || child == 0 {
        return Err(GindexError::ZeroGindex);
    }

    let child_depth = u64::BITS - 1 - child.leading_zeros();
    let child_suffix = child ^ (1_u64 << child_depth);
    append_bits(parent, child_suffix, child_depth)
}

pub fn progressive_chunk_gindex(index: u64) -> Result<u64, GindexError> {
    let mut level = 0_u32;
    let mut group_start = 0_u64;
    let mut group_width = 1_u64;

    loop {
        let group_end = group_start.checked_add(group_width).ok_or(GindexError::Overflow)?;
        if index < group_end {
            break;
        }

        group_start = group_end;
        group_width = group_width.checked_mul(4).ok_or(GindexError::Overflow)?;
        level = level.checked_add(1).ok_or(GindexError::Overflow)?;
    }

    let mut gindex = 2_u64;
    for _ in 0..level {
        gindex = append_bits(gindex, 1, 1)?;
    }

    gindex = append_bits(gindex, 0, 1)?;
    let subtree_depth = level.checked_mul(2).ok_or(GindexError::Overflow)?;
    append_bits(gindex, index - group_start, subtree_depth)
}

pub fn container_field_gindex(field_count: usize, field_index: usize) -> Result<u64, GindexError> {
    if field_count == 0 || field_index >= field_count {
        return Err(GindexError::InvalidContainerField);
    }

    let width = field_count.checked_next_power_of_two().ok_or(GindexError::Overflow)?;
    let gindex = width.checked_add(field_index).ok_or(GindexError::Overflow)?;
    u64::try_from(gindex).map_err(|_| GindexError::Overflow)
}

pub fn receipt_log_address_gindex(path: ResolvedPath) -> Result<u64, GindexError> {
    let ResolvedPath::ReceiptLogAddress { log_index, .. } = path;

    let segments = [
        receipt_logs_gindex(path)?,
        progressive_chunk_gindex(log_index)?,
        container_field_gindex(3, 0)?,
    ];

    segments.into_iter().try_fold(1_u64, compose_gindices)
}

pub(crate) fn receipt_logs_gindex(path: ResolvedPath) -> Result<u64, GindexError> {
    let ResolvedPath::ReceiptLogAddress { receipt_index, .. } = path;
    compose_gindices(progressive_chunk_gindex(receipt_index)?, container_field_gindex(5, 4)?)
}

pub fn branch_positions(mut gindex: u64) -> Result<Vec<u64>, GindexError> {
    if gindex == 0 {
        return Err(GindexError::ZeroGindex);
    }

    let mut positions = Vec::new();
    while gindex > 1 {
        positions.push(gindex ^ 1);
        gindex >>= 1;
    }
    Ok(positions)
}
