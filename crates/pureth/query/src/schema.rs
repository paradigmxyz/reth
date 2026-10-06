#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GindexError {
    ZeroGindex,
    InvalidContainerField,
    Overflow,
}

const fn append_bits(prefix: u128, bits: u128, width: u32) -> Result<u128, GindexError> {
    if width >= u128::BITS || prefix > (u128::MAX >> width) {
        return Err(GindexError::Overflow);
    }

    Ok((prefix << width) | bits)
}

pub const fn compose_gindices(parent: u128, child: u128) -> Result<u128, GindexError> {
    if parent == 0 || child == 0 {
        return Err(GindexError::ZeroGindex);
    }

    let child_depth = u128::BITS - 1 - child.leading_zeros();
    let child_suffix = child ^ (1_u128 << child_depth);
    append_bits(parent, child_suffix, child_depth)
}

pub fn progressive_chunk_gindex(index: u64) -> Result<u128, GindexError> {
    let index = u128::from(index);
    let mut level = 0_u32;
    let mut group_start = 0_u128;
    let mut group_width = 1_u128;

    loop {
        let group_end = group_start.checked_add(group_width).ok_or(GindexError::Overflow)?;
        if index < group_end {
            break;
        }

        group_start = group_end;
        group_width = group_width.checked_mul(4).ok_or(GindexError::Overflow)?;
        level = level.checked_add(1).ok_or(GindexError::Overflow)?;
    }

    let mut gindex = 2_u128;
    for _ in 0..level {
        gindex = append_bits(gindex, 1, 1)?;
    }

    gindex = append_bits(gindex, 0, 1)?;
    let subtree_depth = level.checked_mul(2).ok_or(GindexError::Overflow)?;
    append_bits(gindex, index - group_start, subtree_depth)
}

pub fn container_field_gindex(field_count: usize, field_index: usize) -> Result<u128, GindexError> {
    if field_count == 0 || field_index >= field_count {
        return Err(GindexError::InvalidContainerField);
    }

    let width = field_count.checked_next_power_of_two().ok_or(GindexError::Overflow)?;
    let gindex = width.checked_add(field_index).ok_or(GindexError::Overflow)?;
    Ok(gindex as u128)
}

pub fn branch_positions(mut gindex: u128) -> Result<Vec<u128>, GindexError> {
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
