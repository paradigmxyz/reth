//! Golden vectors: the mapping math asserted against go-ethereum's `core/filtermaps`.
//!
//! These are the crate's primary correctness gate. Every table in [`vectors`] was produced by
//! running Geth's own unexported mapping functions over fixed inputs, so a passing run means the
//! port agrees with the oracle rather than merely with itself. See the header of `vectors.rs` for
//! the pinned Geth commit and the regeneration procedure.

use alloy_primitives::{Address, B256};
use reth_filter_maps::{address_value, topic_value, DEFAULT_PARAMS, RANGE_TEST_PARAMS};

// The tables are `pub` as the generator emits them. Regenerating overwrites the file wholesale, so
// nothing there is hand-maintained beyond rustfmt's wrapping.
#[allow(unreachable_pub)]
mod vectors;

/// Parses a `0x`-prefixed 32-byte hex string from a vector table.
fn parse_hash(hex: &str) -> B256 {
    hex.parse().expect("golden vector holds a valid 32-byte hex string")
}

#[test]
fn address_values_match_geth() {
    for (input, expected) in vectors::ADDRESS_VALUES {
        let address: Address = input.parse().expect("golden vector holds a valid address");
        assert_eq!(address_value(address), parse_hash(expected), "address_value({input})");
    }
}

#[test]
fn topic_values_match_geth() {
    for (input, expected) in vectors::TOPIC_VALUES {
        assert_eq!(topic_value(parse_hash(input)), parse_hash(expected), "topic_value({input})");
    }
}

#[test]
fn row_indices_match_geth() {
    for (value, map_index, layer_index, expected) in vectors::ROW_INDEX {
        assert_eq!(
            DEFAULT_PARAMS.row_index(*map_index, *layer_index, parse_hash(value)),
            *expected,
            "row_index({value}, {map_index}, {layer_index})"
        );
    }
}

#[test]
fn column_indices_match_geth() {
    for (value, log_value_index, expected) in vectors::COLUMN_INDEX {
        assert_eq!(
            DEFAULT_PARAMS.column_index(*log_value_index, parse_hash(value)),
            *expected,
            "column_index({value}, {log_value_index})"
        );
    }
}

#[test]
fn max_row_lengths_match_geth() {
    let cases = [
        (&DEFAULT_PARAMS, vectors::MAX_ROW_LENGTH_DEFAULT),
        (&RANGE_TEST_PARAMS, vectors::MAX_ROW_LENGTH_RANGE_TEST),
    ];

    for (params, table) in cases {
        for (layer_index, expected) in table {
            assert_eq!(
                params.max_row_length(*layer_index),
                *expected,
                "max_row_length({layer_index})"
            );
        }
    }
}

#[test]
fn masked_map_indices_match_geth() {
    let cases = [
        (&DEFAULT_PARAMS, vectors::MASKED_MAP_INDEX_DEFAULT),
        (&RANGE_TEST_PARAMS, vectors::MASKED_MAP_INDEX_RANGE_TEST),
    ];

    for (params, table) in cases {
        for (map_index, layer_index, expected) in table {
            assert_eq!(
                params.masked_map_index(*map_index, *layer_index),
                *expected,
                "masked_map_index({map_index}, {layer_index})"
            );
        }
    }
}

/// Translation between a map index and the epoch containing it. A wrong shift here moves an epoch
/// boundary, which misplaces whole runs of rows rather than single values.
#[test]
fn epoch_helpers_match_geth() {
    let cases = [
        (&DEFAULT_PARAMS, vectors::EPOCH_HELPERS_DEFAULT),
        (&RANGE_TEST_PARAMS, vectors::EPOCH_HELPERS_RANGE_TEST),
    ];

    for (params, table) in cases {
        for (map_index, epoch, first, last) in table {
            assert_eq!(params.map_epoch(*map_index), *epoch, "map_epoch({map_index})");
            assert_eq!(params.first_epoch_map(*epoch), *first, "first_epoch_map({epoch})");
            assert_eq!(params.last_epoch_map(*epoch), *last, "last_epoch_map({epoch})");
        }
    }
}

/// The split of a map index into its base row group and the offset within that group, which is how
/// base rows are keyed as database entries.
#[test]
fn map_group_helpers_match_geth() {
    for (map_index, group_index, group_offset) in vectors::MAP_GROUP_DEFAULT {
        assert_eq!(
            DEFAULT_PARAMS.map_group_index(*map_index),
            *group_index,
            "map_group_index({map_index})"
        );
        assert_eq!(
            DEFAULT_PARAMS.map_group_offset(*map_index),
            *group_offset,
            "map_group_offset({map_index})"
        );
    }
}

/// `base_row_length` per the `// sanity` comments at the foot of `vectors.rs`. Alone among the
/// derived fields it involves real arithmetic — integer division that can silently floor to zero —
/// where the shift-derived fields are already pinned by the mapping tables above.
#[test]
fn base_row_lengths_match_geth() {
    assert_eq!(DEFAULT_PARAMS.base_row_length(), 8);
    assert_eq!(RANGE_TEST_PARAMS.base_row_length(), 1);
}
