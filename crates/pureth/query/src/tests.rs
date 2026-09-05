use super::*;
use crate::schema::validate_runtime_lengths;
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
fn resolver_accepts_only_the_v0_receipt_log_address_shape() {
    let resolved = resolve_v0(&parse_path("[5].logs[12].address").unwrap());
    assert_eq!(resolved, Ok(ResolvedPath::ReceiptLogAddress { receipt_index: 5, log_index: 12 }));

    for path in [
        ".status",
        "[0][1]",
        "[0].logs[0].topics[0]",
        "[0].logs[0].data[0]",
        "[0].logs.address",
        "[0].logs[0].address.extra",
    ] {
        let tokens = parse_path(path).unwrap();
        assert_eq!(resolve_v0(&tokens), Err(UnsupportedPath), "path: {path}");
    }
}

#[test]
fn runtime_bounds_are_checked_after_resolution() {
    let receipt_log_counts = [1, 3];

    assert_eq!(
        validate_runtime_bounds(
            ResolvedPath::ReceiptLogAddress { receipt_index: 1, log_index: 2 },
            &receipt_log_counts,
        ),
        Ok((1, 2))
    );
    assert_eq!(
        validate_runtime_bounds(
            ResolvedPath::ReceiptLogAddress { receipt_index: 2, log_index: 0 },
            &receipt_log_counts,
        ),
        Err(BoundsError::ReceiptOutOfBounds)
    );
    assert_eq!(
        validate_runtime_bounds(
            ResolvedPath::ReceiptLogAddress { receipt_index: 0, log_index: 1 },
            &receipt_log_counts,
        ),
        Err(BoundsError::LogOutOfBounds)
    );
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
fn sample_gindex_is_composed_from_schema_segments() {
    let path = ResolvedPath::ReceiptLogAddress { receipt_index: 0, log_index: 0 };

    assert_eq!(progressive_chunk_gindex(0), Ok(4));
    assert_eq!(container_field_gindex(5, 4), Ok(12));
    assert_eq!(container_field_gindex(3, 0), Ok(4));
    assert_eq!(compose_gindices(1, 4), Ok(4));
    assert_eq!(compose_gindices(4, 12), Ok(36));
    assert_eq!(compose_gindices(36, 4), Ok(144));
    assert_eq!(compose_gindices(144, 4), Ok(576));
    assert_eq!(receipt_log_address_gindex(path), Ok(576));
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
    assert_eq!(compose_gindices(u64::MAX, 2), Err(GindexError::Overflow));
    assert_eq!(container_field_gindex(0, 0), Err(GindexError::InvalidContainerField));
    assert_eq!(container_field_gindex(3, 3), Err(GindexError::InvalidContainerField));
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

#[test]
fn envelope_binds_schema_path_value_and_proof() {
    let case = crate::vector_records::load_proof_case(
        include_bytes!("../test-data/fixtures/v0/singleton_baseline/fixture.json"),
        include_bytes!("../test-data/fixtures/v0/singleton_baseline/proof.json"),
    )
    .unwrap();

    assert_eq!(
        verify_receipt_log_address(
            &case.proof.schema_id,
            &case.proof.path,
            &case.selected_address,
            &case.branch,
            case.root,
        ),
        Ok(())
    );

    assert_eq!(
        verify_receipt_log_address(
            "other-schema",
            &case.proof.path,
            &case.selected_address,
            &case.branch,
            case.root,
        ),
        Err(EnvelopeError::WrongSchema)
    );

    assert_eq!(
        verify_receipt_log_address(
            SCHEMA_ID,
            "[0].logs[0].topics[0]",
            &case.selected_address,
            &case.branch,
            case.root,
        ),
        Err(EnvelopeError::UnsupportedPath)
    );

    assert_eq!(
        verify_receipt_log_address(
            SCHEMA_ID,
            "[0].logs[0].address",
            &[0x11; 19],
            &case.branch,
            case.root,
        ),
        Err(EnvelopeError::InvalidValue(InvalidAddressLength { actual: 19 }))
    );

    assert_eq!(
        validate_runtime_lengths(
            ResolvedPath::ReceiptLogAddress { receipt_index: 0, log_index: 1 },
            1,
            1,
        ),
        Err(BoundsError::LogOutOfBounds)
    );
    assert_eq!(
        validate_runtime_lengths(
            ResolvedPath::ReceiptLogAddress { receipt_index: 1, log_index: 0 },
            1,
            1,
        ),
        Err(BoundsError::ReceiptOutOfBounds)
    );
}
