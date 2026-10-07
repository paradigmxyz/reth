use super::*;
use alloy_primitives::B256;

#[test]
fn parser_accepts_valid_syntax() {
    let cases = [
        (
            "[0].logs[0].address",
            vec![
                PathToken::Index(0),
                PathToken::Field("logs".into()),
                PathToken::Index(0),
                PathToken::Field("address".into()),
            ],
        ),
        (
            "[5].logs[12].address",
            vec![
                PathToken::Index(5),
                PathToken::Field("logs".into()),
                PathToken::Index(12),
                PathToken::Field("address".into()),
            ],
        ),
        (".status", vec![PathToken::Field("status".into())]),
        ("[0][1]", vec![PathToken::Index(0), PathToken::Index(1)]),
        (".field_2", vec![PathToken::Field("field_2".into())]),
    ];

    for (path, expected) in cases {
        assert_eq!(parse_path(path), Ok(expected), "path: {path}");
    }
}

#[test]
fn parser_rejects_invalid_syntax() {
    let cases = [
        "",
        ".",
        "[0",
        "0]",
        "[-1]",
        "[01]",
        "[]",
        "[ 0]",
        "[*]",
        "[1:3]",
        ".logs.",
        ".1field",
        ".log-address",
        "[18446744073709551616]",
    ];

    for path in cases {
        assert!(parse_path(path).is_err(), "path unexpectedly passed: {path}");
    }
}

#[test]
fn parser_bounds_path_bytes_before_allocating_tokens() {
    let accepted = format!(".{}", "a".repeat(255));
    let too_long = format!(".{}", "a".repeat(256));

    assert!(parse_path(&accepted).is_ok());
    assert_eq!(parse_path(&too_long), Err(ParseError::PathTooLong));
}

#[test]
fn progressive_chunk_mapping_matches_the_pinned_reference_ranges() {
    let cases = [
        (0, 4),
        (1, 40),
        (4, 43),
        (5, 352),
        (20, 367),
        (21, 2944),
        (84, 3007),
        (85, 24064),
        (340, 24319),
    ];

    for (index, expected) in cases {
        assert_eq!(progressive_chunk_gindex(index), Ok(expected));
    }
}

#[test]
fn branch_positions_are_immediate_sibling_first() {
    assert_eq!(branch_positions(576), Ok(vec![577, 289, 145, 73, 37, 19, 8, 5, 3]));
    assert_eq!(branch_positions(1), Ok(vec![]));
    assert_eq!(branch_positions(0), Err(GindexError::ZeroGindex));
}

#[test]
fn gindex_errors_are_explicit() {
    assert_eq!(compose_gindices(1, 0), Err(GindexError::ZeroGindex));
    assert_eq!(compose_gindices(u128::MAX, 2), Err(GindexError::Overflow));
    assert_eq!(container_field_gindex(0, 0), Err(GindexError::InvalidContainerField));
    assert_eq!(container_field_gindex(3, 3), Err(GindexError::InvalidContainerField));
}

#[test]
fn composition_crosses_bit_64_and_stops_at_depth_127() {
    assert_eq!(compose_gindices(1_u128 << 63, 3), Ok((1_u128 << 64) | 1));
    assert_eq!(compose_gindices(1_u128 << 126, 2), Ok(1_u128 << 127));
    assert_eq!(compose_gindices(1_u128 << 127, 1), Ok(1_u128 << 127));
    assert_eq!(compose_gindices(1_u128 << 127, 2), Err(GindexError::Overflow));
    assert_eq!(compose_gindices(u128::MAX, 1), Ok(u128::MAX));
    assert_eq!(compose_gindices(u128::MAX, 3), Err(GindexError::Overflow));
    assert_eq!(branch_positions(u128::MAX).unwrap().len(), 127);
    assert_eq!(branch_positions(u128::MAX).unwrap()[126], 2);
}

#[test]
fn progressive_positions_keep_large_logical_indices_separate() {
    for (index, expected) in [
        (1_u64 << 42, 55_340_226_357_066_640_043_u128),
        (1_u64 << 63, 475_368_975_051_766_994_759_462_857_387_u128),
        (u64::MAX, 475_368_975_060_990_366_796_317_633_194_u128),
    ] {
        assert_eq!(progressive_chunk_gindex(index), Ok(expected));
    }
}

#[test]
fn address_target_is_right_padded_to_one_chunk() {
    let address = [0x11_u8; 20];
    let node = address_target_node(&address).unwrap();

    assert_eq!(&node[..20], &address);
    assert_eq!(&node[20..], &[0_u8; 12]);
    assert_eq!(address_target_node(&address[..19]), Err(InvalidAddressLength { actual: 19 }));

    let too_long = [0x11_u8; 21];
    assert_eq!(address_target_node(&too_long), Err(InvalidAddressLength { actual: 21 }));
}

fn synthetic_branch() -> Vec<B256> {
    (1_u8..=9).map(B256::repeat_byte).collect()
}

const SYNTHETIC_ROOT: [u8; 32] = [
    0x11, 0x84, 0xa6, 0xbd, 0x39, 0x76, 0xa6, 0xca, 0xc9, 0x45, 0x17, 0x27, 0x77, 0x7a, 0xb1, 0x5c,
    0xc1, 0xd6, 0x36, 0xe1, 0x19, 0x79, 0x74, 0x04, 0xcf, 0x84, 0x0a, 0xe6, 0x19, 0x5d, 0xf1, 0x02,
];

#[test]
fn verifier_accepts_the_independently_computed_synthetic_branch() {
    let target = address_target_node(&[0x11; 20]).unwrap();

    assert_eq!(verify_branch(target, 576, &synthetic_branch(), B256::from(SYNTHETIC_ROOT)), Ok(()));
}

#[test]
fn verifier_rejects_single_input_mutations() {
    let target = address_target_node(&[0x11; 20]).unwrap();
    let branch = synthetic_branch();

    let mut wrong_target = target;
    wrong_target[0] ^= 1;
    assert_eq!(
        verify_branch(wrong_target, 576, &branch, B256::from(SYNTHETIC_ROOT)),
        Err(ProofError::RootMismatch)
    );

    let mut wrong_sibling = branch.clone();
    wrong_sibling[3][0] ^= 1;
    assert_eq!(
        verify_branch(target, 576, &wrong_sibling, B256::from(SYNTHETIC_ROOT)),
        Err(ProofError::RootMismatch)
    );

    let mut wrong_order = branch.clone();
    wrong_order.swap(0, 1);
    assert_eq!(
        verify_branch(target, 576, &wrong_order, B256::from(SYNTHETIC_ROOT)),
        Err(ProofError::RootMismatch)
    );

    assert_eq!(
        verify_branch(target, 576, &branch[..8], B256::from(SYNTHETIC_ROOT)),
        Err(ProofError::WrongBranchLength { expected: 9, actual: 8 })
    );

    let mut too_long = branch.clone();
    too_long.push(B256::repeat_byte(10));
    assert_eq!(
        verify_branch(target, 576, &too_long, B256::from(SYNTHETIC_ROOT)),
        Err(ProofError::WrongBranchLength { expected: 9, actual: 10 })
    );

    assert_eq!(
        verify_branch(target, 577, &branch, B256::from(SYNTHETIC_ROOT)),
        Err(ProofError::RootMismatch)
    );

    let mut wrong_root = B256::from(SYNTHETIC_ROOT);
    wrong_root[0] ^= 1;
    assert_eq!(verify_branch(target, 576, &branch, wrong_root), Err(ProofError::RootMismatch));
    assert_eq!(
        verify_branch(target, 0, &[], B256::from(SYNTHETIC_ROOT)),
        Err(ProofError::ZeroGindex)
    );
}
