// GENERATED from go-ethereum core/filtermaps. DO NOT EDIT.
// Geth commit: ca1f2e4d38f4e94676981bb9251239a5d490b004
//
// Regenerate: the mapping functions are unexported, so this is produced by an
// in-package Go test placed at core/filtermaps/ in a go-ethereum checkout at the
// commit above. It calls sanitize() on DefaultParams and RangeTestParams (which
// runs deriveFields; the mapping functions read those derived fields), then walks
// addressValue, topicValue, rowIndex, columnIndex, maxRowLength, maskedMapIndex,
// mapEpoch, firstEpochMap, lastEpochMap, mapGroupIndex and mapGroupOffset over the
// inputs recorded in each table below, and prints them as Rust consts:
//   go test -run TestGenGolden -v
// The generator (gen_golden_test.go) is not part of this repository; see the
// FilterMaps tracking issue, https://github.com/paradigmxyz/reth/issues/16999.
// The description above is sufficient to rewrite it.
//
// Params are DEFAULT unless a table name says RANGE_TEST.

pub const ADDRESS_VALUES: &[(&str, &str)] = &[
    (
        "0x0000000000000000000000000000000000000000",
        "0xde47c9b27eb8d300dbb5f2c353e632c393262cf06340c4fa7f1b40c4cbd36f90",
    ),
    (
        "0x0000000000000000000000000000000000000001",
        "0xe9ff0e6e6de95da56ff09f4e3e0f481d67585f0a68aafdeef0f86f7b8533ce17",
    ),
    (
        "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        "0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7",
    ),
    (
        "0xffffffffffffffffffffffffffffffffffffffff",
        "0x9a8dcd3f9ff7aa3114e141f03c12989d363ea81fd74c02eea63c5f41489cb17a",
    ),
];

pub const TOPIC_VALUES: &[(&str, &str)] = &[
    (
        "0x0000000000000000000000000000000000000000000000000000000000000000",
        "0x66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925",
    ),
    (
        "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
        "0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21",
    ),
    (
        "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "0xaf9613760f72635fbdb44a5a0a63c39f12af30f950a6ee5c971be188e89c4051",
    ),
];

// (value_hash, map_index, layer_index) -> row_index
pub const ROW_INDEX: &[(&str, u32, u32, u32)] = &[
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 0, 0, 60735),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 0, 1, 44939),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 0, 2, 1632),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 0, 3, 43699),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 0, 4, 23573),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 0, 5, 41240),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 0, 6, 54186),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 5, 0, 60735),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 5, 1, 44939),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 5, 2, 20910),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 5, 3, 20624),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 5, 4, 19378),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 5, 5, 51827),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 5, 6, 39766),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1023, 0, 60735),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1023, 1, 6915),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1023, 2, 23622),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1023, 3, 5041),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1023, 4, 14521),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1023, 5, 64914),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1023, 6, 40761),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1024, 0, 55491),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1024, 1, 23525),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1024, 2, 49541),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1024, 3, 40278),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1024, 4, 31744),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1024, 5, 46909),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1024, 6, 4559),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1025, 0, 55491),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1025, 1, 23525),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1025, 2, 49541),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1025, 3, 1943),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1025, 4, 22034),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1025, 5, 4947),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1025, 6, 25824),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 2048, 0, 20014),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 2048, 1, 25221),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 2048, 2, 35488),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 2048, 3, 36999),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 2048, 4, 29276),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 2048, 5, 22937),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 2048, 6, 60188),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 0, 0, 23957),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 0, 1, 29384),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 0, 2, 35833),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 0, 3, 35803),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 0, 4, 17029),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 0, 5, 28169),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 0, 6, 35649),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 5, 0, 23957),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 5, 1, 29384),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 5, 2, 21157),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 5, 3, 8109),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 5, 4, 6696),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 5, 5, 58111),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 5, 6, 21950),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1023, 0, 23957),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1023, 1, 13614),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1023, 2, 51384),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1023, 3, 15491),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1023, 4, 12222),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1023, 5, 39978),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1023, 6, 38199),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1024, 0, 12855),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1024, 1, 64577),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1024, 2, 61452),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1024, 3, 45249),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1024, 4, 41694),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1024, 5, 5987),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1024, 6, 28176),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1025, 0, 12855),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1025, 1, 64577),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1025, 2, 61452),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1025, 3, 39151),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1025, 4, 39809),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1025, 5, 32047),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1025, 6, 36199),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 2048, 0, 41945),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 2048, 1, 7603),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 2048, 2, 45152),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 2048, 3, 42698),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 2048, 4, 52538),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 2048, 5, 44529),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 2048, 6, 62138),
];

// (value_hash, log_value_index) -> column_index
pub const COLUMN_INDEX: &[(&str, u64, u32)] = &[
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 0, 3),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1, 289),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 65535, 16777105),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 65536, 97),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 65537, 477),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 131072, 85),
    ("0xe7fb736fa7ed3fb551fd9169fe0f055fb610c67f03d5fb3fe39790079bfd3ec7", 1234567, 14059285),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 0, 210),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1, 384),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 65535, 16777194),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 65536, 174),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 65537, 428),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 131072, 114),
    ("0xaea00b5d38687a0ed7524ecbe08a98d4154576593ff13d4c725db7fbbe46fe21", 1234567, 14059325),
];

// layer -> max_row_length (DEFAULT)
pub const MAX_ROW_LENGTH_DEFAULT: &[(u32, u32)] =
    &[(0, 8), (1, 128), (2, 2048), (3, 8192), (4, 8192), (5, 8192), (6, 8192)];

// layer -> max_row_length (RANGE_TEST)
pub const MAX_ROW_LENGTH_RANGE_TEST: &[(u32, u32)] =
    &[(0, 1), (1, 1), (2, 1), (3, 1), (4, 1), (5, 1), (6, 1)];

// (map_index, layer) -> masked_map_index (DEFAULT)
pub const MASKED_MAP_INDEX_DEFAULT: &[(u32, u32, u32)] = &[
    (0, 0, 0),
    (0, 1, 0),
    (0, 2, 0),
    (0, 3, 0),
    (0, 4, 0),
    (0, 5, 0),
    (0, 6, 0),
    (5, 0, 0),
    (5, 1, 0),
    (5, 2, 4),
    (5, 3, 5),
    (5, 4, 5),
    (5, 5, 5),
    (5, 6, 5),
    (1023, 0, 0),
    (1023, 1, 960),
    (1023, 2, 1020),
    (1023, 3, 1023),
    (1023, 4, 1023),
    (1023, 5, 1023),
    (1023, 6, 1023),
    (1024, 0, 1024),
    (1024, 1, 1024),
    (1024, 2, 1024),
    (1024, 3, 1024),
    (1024, 4, 1024),
    (1024, 5, 1024),
    (1024, 6, 1024),
    (1025, 0, 1024),
    (1025, 1, 1024),
    (1025, 2, 1024),
    (1025, 3, 1025),
    (1025, 4, 1025),
    (1025, 5, 1025),
    (1025, 6, 1025),
    (2048, 0, 2048),
    (2048, 1, 2048),
    (2048, 2, 2048),
    (2048, 3, 2048),
    (2048, 4, 2048),
    (2048, 5, 2048),
    (2048, 6, 2048),
];

// (map_index, layer) -> masked_map_index (RANGE_TEST)
pub const MASKED_MAP_INDEX_RANGE_TEST: &[(u32, u32, u32)] = &[
    (0, 0, 0),
    (0, 1, 0),
    (0, 2, 0),
    (0, 3, 0),
    (0, 4, 0),
    (0, 5, 0),
    (0, 6, 0),
    (1, 0, 1),
    (1, 1, 1),
    (1, 2, 1),
    (1, 3, 1),
    (1, 4, 1),
    (1, 5, 1),
    (1, 6, 1),
    (2, 0, 2),
    (2, 1, 2),
    (2, 2, 2),
    (2, 3, 2),
    (2, 4, 2),
    (2, 5, 2),
    (2, 6, 2),
    (7, 0, 7),
    (7, 1, 7),
    (7, 2, 7),
    (7, 3, 7),
    (7, 4, 7),
    (7, 5, 7),
    (7, 6, 7),
];

// map_index -> (map_epoch, first_epoch_map(that epoch), last_epoch_map(that epoch)) (DEFAULT)
pub const EPOCH_HELPERS_DEFAULT: &[(u32, u32, u32, u32)] = &[
    (0, 0, 0, 1023),
    (1, 0, 0, 1023),
    (1023, 0, 0, 1023),
    (1024, 1, 1024, 2047),
    (1025, 1, 1024, 2047),
    (2047, 1, 1024, 2047),
    (2048, 2, 2048, 3071),
    (1048575, 1023, 1047552, 1048575),
    (1048576, 1024, 1048576, 1049599),
];

// map_index -> (map_epoch, first_epoch_map(that epoch), last_epoch_map(that epoch)) (RANGE_TEST)
pub const EPOCH_HELPERS_RANGE_TEST: &[(u32, u32, u32, u32)] =
    &[(0, 0, 0, 0), (1, 1, 1, 1), (2, 2, 2, 2), (7, 7, 7, 7), (1024, 1024, 1024, 1024)];

// map_index -> (map_group_index, map_group_offset) (DEFAULT)
pub const MAP_GROUP_DEFAULT: &[(u32, u32, u32)] = &[
    (0, 0, 0),
    (1, 0, 1),
    (31, 0, 31),
    (32, 32, 0),
    (33, 32, 1),
    (63, 32, 31),
    (64, 64, 0),
    (1000, 992, 8),
    (4294967295, 4294967264, 31),
];

// sanity DEFAULT:    base_row_length=8 map_height=65536 values_per_map=65536 maps_per_epoch=1024
// sanity RANGE_TEST: base_row_length=1 map_height=16 values_per_map=1 maps_per_epoch=1
