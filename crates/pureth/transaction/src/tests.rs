use super::*;
use alloy_consensus::{Signed, TxEip1559};
use alloy_eips::{eip2930::AccessListItem, eip7702::Authorization};
use alloy_primitives::{b256, uint, Address};

#[test]
fn legacy_call_bytes_and_hashes_match_pinned_reference() {
    for (selector, rlp, ssz, signing_hash, transaction_hash, object_root) in [
        (
            1_u8,
            "f86780862d79883d2000825208945df9b87991262f6ba471f09758cde1c0fc1de734827a69801ca088ff6cf0fefd94db46111149ae4bfc179e9b94721fffd821d38d16464b3f71d0a045e0aff800961cfce805daef7016b9b675c137a6a41a548f7b60a3484c06a33a",
            "08000000720000000100000000000000000000203d88792d000000000000000000000000000000000000000000000000000008520000000000005df9b87991262f6ba471f09758cde1c0fc1de734697a000000000000000000000000000000000000000000000000000000000000690000000088ff6cf0fefd94db46111149ae4bfc179e9b94721fffd821d38d16464b3f71d045e0aff800961cfce805daef7016b9b675c137a6a41a548f7b60a3484c06a33a01",
            b256!("19b1e28c14f33e74b96b88eba97d4a4fc8a97638d72e972310025b7e1189b049"),
            b256!("5c504ed432cb51138bcf09aa5e8a410dd4a1e204ef84bfed1be16dfba1b22060"),
            b256!("bef7b49904562eac794ae36b9e675e46d638bc4121453f9a46bbe03473d1ab87"),
        ),
        (
            1_u8,
            "f86e8243eb850df847580082c35094df190dc7190dfba737d7777a163445b7fff161338806113a84987be800801ca03b08715b4403c792b8c7567edea634088bedcd7f60d9352b1f16c69830f3afd5a010b9afb67d2ec8b956f0e1dbc07eb79152904f3a7bf789fc869db56320adfe09",
            "08000000720000000100eb43000000000000005847f80d00000000000000000000000000000000000000000000000000000050c3000000000000df190dc7190dfba737d7777a163445b7fff1613300e87b98843a110600000000000000000000000000000000000000000000000069000000003b08715b4403c792b8c7567edea634088bedcd7f60d9352b1f16c69830f3afd510b9afb67d2ec8b956f0e1dbc07eb79152904f3a7bf789fc869db56320adfe0901",
            b256!("05887873a4fc88813466382673be81701adc8f564f7bcd17e2ce974a43100d04"),
            b256!("e9e91f1ee4b56c0df2e9f06c2b8c27c6076195a88a7b8537ba8313d80e6f124e"),
            b256!("b5fd697f31a68d85d6502a137b11548302dffa23b1954aa6b12d105599a82ad2"),
        ),
        (
            3_u8,
            "f88b822ecd8509839089a083015f909484654be796dad370032391d5479f8f1fd9ddd14e80a4d508e62389272d541e4168e5303b5838dcef5f9ac769a057f8dc6aabce7ec1e69b7ce5e625a078e88f9c69d217c76d92d5472f5812501021342f0054178c0579ee532dc2a218a0484723f0933633a6c213fc201ee3aecbe919579377b11a79acd887950a068e33",
            "08000000b600000003000100000000000000000000000000000000000000000000000000000000000000cd2e000000000000a089908309000000000000000000000000000000000000000000000000000000905f01000000000084654be796dad370032391d5479f8f1fd9ddd14e000000000000000000000000000000000000000000000000000000000000000089000000d508e62389272d541e4168e5303b5838dcef5f9ac769a057f8dc6aabce7ec1e69b7ce5e60078e88f9c69d217c76d92d5472f5812501021342f0054178c0579ee532dc2a218484723f0933633a6c213fc201ee3aecbe919579377b11a79acd887950a068e3300",
            b256!("3e4c94ee2d68abe91ab5c97ad1111f5367a8cbdc8e907850ff9a297a11755441"),
            b256!("dc81918bf78322ce017c592e81c855f40bb96bd82da9779167de1de109962be6"),
            b256!("ad2afaa33122dd0c7a4c18f600f18d256cfe503dab5b2e24e0029455109f89cd"),
        ),
    ] {
        let rlp = alloy_primitives::hex::decode(rlp).unwrap();
        let ssz = alloy_primitives::hex::decode(ssz).unwrap();
        let transaction = TransactionSsz::from_rlp(&rlp).unwrap();
        assert_eq!(transaction.selector(), selector);
        assert_eq!(encode_transaction(&transaction).unwrap(), ssz);
        assert_eq!(decode_transaction(&ssz).unwrap(), transaction);
        assert_eq!(transaction.original_rlp().unwrap().as_ref(), rlp);
        assert_eq!(transaction.original_signing_hash().unwrap(), signing_hash);
        assert_eq!(transaction.original_transaction_hash().unwrap(), transaction_hash);
        let snapshot = TransactionSnapshot::build(vec![transaction]).unwrap();
        assert_eq!(snapshot.transaction_tree(0).unwrap().root(), object_root);
    }
}

fn signature() -> Signature {
    Signature::new(
        uint!(0x88ff6cf0fefd94db46111149ae4bfc179e9b94721fffd821d38d16464b3f71d0_U256),
        uint!(0x45e0aff800961cfce805daef7016b9b675c137a6a41a548f7b60a3484c06a33a_U256),
        true,
    )
}

fn transaction(chain_id: Option<U256>, to: TxKind) -> TransactionSsz {
    TransactionSsz::new(
        LegacyPayload {
            chain_id,
            nonce: 9,
            gas_price: U256::from(20_000_000_000_u64),
            gas_limit: 53_000,
            to,
            value: U256::from(7),
            input: Bytes::from(vec![0x01, 0x02, 0x03]),
        },
        signature(),
    )
    .unwrap()
}

fn four_variants() -> Vec<TransactionSsz> {
    let call = TxKind::Call(Address::repeat_byte(0x11));
    vec![
        transaction(None, call),
        transaction(None, TxKind::Create),
        transaction(Some(U256::from(1)), call),
        transaction(Some(U256::from(1)), TxKind::Create),
    ]
}

#[test]
fn four_variants_preserve_canonical_bytes_rlp_and_hashes() {
    let roots = [
        b256!("73ceedc325e607a90ca880e0efed4eca06837e5083c6f26034766c10b7bfc21b"),
        b256!("cfdbf09099661b05b05b46dd65560055c7d1290d5f98252748661ad21906d47a"),
        b256!("b5ffccfdabe1828d188d15196b5a23ea4eac490374301134d2b29e04b6a04aca"),
        b256!("cc0652f06794faa3f6c4959bcb9945886ebea807f8fc0c55aa47086d03c32ae4"),
    ];
    for (index, (transaction, expected_root)) in four_variants().into_iter().zip(roots).enumerate()
    {
        assert_eq!(usize::from(transaction.selector()), index + 1);
        let bytes = encode_transaction(&transaction).unwrap();
        let decoded = decode_transaction(&bytes).unwrap();
        assert_eq!(decoded, transaction);
        assert_eq!(encode_transaction(&decoded).unwrap(), bytes);

        let original = transaction.to_envelope().unwrap();
        let rlp = transaction.original_rlp().unwrap();
        assert_eq!(TransactionSsz::from_envelope(&original).unwrap(), transaction);
        assert_eq!(TransactionSsz::from_rlp(&rlp).unwrap(), transaction);
        assert_eq!(transaction.original_transaction_hash().unwrap(), *original.tx_hash());
        assert_eq!(transaction.original_signing_hash().unwrap(), original.signature_hash());
        let snapshot = TransactionSnapshot::build(vec![transaction]).unwrap();
        assert_eq!(snapshot.transaction_tree(0).unwrap().root(), expected_root);
    }
}

#[test]
fn optional_fields_distinguish_absence_from_zero() {
    let variants = vec![
        transaction(None, TxKind::Create),
        transaction(None, TxKind::Call(Address::ZERO)),
        transaction(Some(U256::ZERO), TxKind::Create),
        transaction(Some(U256::ZERO), TxKind::Call(Address::ZERO)),
    ];
    let snapshot = TransactionSnapshot::build(variants).unwrap();
    for left in 0..4 {
        for right in left + 1..4 {
            assert_ne!(
                snapshot.transaction_tree(left).unwrap().root(),
                snapshot.transaction_tree(right).unwrap().root(),
            );
        }
    }
}

#[test]
fn list_snapshot_preserves_order_and_retained_transaction_access() {
    for (count, expected_root) in [
        (0, b256!("f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b")),
        (1, b256!("94ac38107e10f8495dedf4621d9100d566bb376b8adb4850425a003b5cef479d")),
        (4, b256!("e288b5a499b31a13788b9d59b5cf9ef4e2d241f4c60b07e7521b0ec8501b1549")),
        (5, b256!("4d4084a6276735bdb0ef4b26f29423c7a084d84d9c6008b1c57262429097be39")),
        (6, b256!("7e893ad228751090c7bd8242f7c3abee7a4ae613a1b1d299b780f4a2bffa554d")),
        (21, b256!("a4c07072f7b1bf976ea730e94e0164c4f34b317db9e53566196d21cac9c173a3")),
        (22, b256!("cf70fb2685713fc946c67206cb9f8b77ea2e217bcad6eb0d479b8292c60a1ca4")),
    ] {
        let transactions = (0..count)
            .map(|nonce| {
                let TransactionPayload::Legacy(mut payload) =
                    four_variants()[nonce % 4].payload().clone()
                else {
                    panic!("expected legacy payload");
                };
                payload.nonce = u64::try_from(nonce).unwrap();
                TransactionSsz::new(payload, signature()).unwrap()
            })
            .collect::<Vec<_>>();
        let snapshot = TransactionSnapshot::build(transactions.clone()).unwrap();
        assert_eq!(snapshot.root(), expected_root);
        let decoded = TransactionSnapshot::from_ssz(snapshot.serialized()).unwrap();
        assert_eq!(decoded.transactions(), transactions);
        assert_eq!(decoded.root(), snapshot.root());
        assert_eq!(snapshot.root(), snapshot.tree().root());
        assert!(snapshot.transaction_tree(count).is_none());
        assert!(snapshot.transaction_tree(usize::MAX).is_none());

        for (index, transaction) in transactions.iter().enumerate() {
            let singleton = TransactionSnapshot::build(vec![transaction.clone()]).unwrap();
            let node = snapshot.transaction_tree(index).unwrap();
            assert_eq!(node.root(), singleton.transaction_tree(0).unwrap().root());
            assert!(node.children().is_some());
        }

        if count > 1 {
            let mut reordered = transactions;
            reordered.reverse();
            assert_ne!(TransactionSnapshot::build(reordered).unwrap().root(), snapshot.root());
        }
    }
    assert_eq!(
        TransactionSnapshot::build(Vec::new()).unwrap().root(),
        b256!("f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b"),
    );
}

#[test]
fn mutations_change_the_retained_commitment() {
    let original = four_variants().remove(0);
    let root = TransactionSnapshot::build(vec![original.clone()]).unwrap().root();
    let TransactionPayload::Legacy(mut payload) = original.payload().clone() else {
        panic!("expected legacy payload");
    };
    payload.input = Bytes::from(vec![0x01, 0x02, 0x04]);
    assert_ne!(
        TransactionSnapshot::build(vec![TransactionSsz::new(payload, signature()).unwrap()])
            .unwrap()
            .root(),
        root,
    );
    let changed =
        TransactionSsz::new(original.payload().clone(), signature().with_parity(false)).unwrap();
    assert_ne!(TransactionSnapshot::build(vec![changed]).unwrap().root(), root);
}

#[test]
fn malformed_transaction_encodings_are_rejected_without_panics() {
    for transaction in four_variants() {
        let bytes = encode_transaction(&transaction).unwrap();
        for length in 0..bytes.len() {
            assert!(decode_transaction(&bytes[..length]).is_err());
        }
        let fixed_length = match transaction.selector() {
            1 => 105,
            2 => 85,
            3 => 137,
            4 => 117,
            _ => unreachable!(),
        };
        let signature_offset = bytes.len() - 66;
        for (position, replacement) in [
            (0, 7),
            (4, 0),
            (8, 0),
            (8, 5),
            (9, 1),
            (9 + fixed_length - 4, 0),
            (signature_offset, 1),
            (bytes.len() - 1, 2),
        ] {
            let mut invalid = bytes.clone();
            invalid[position] = replacement;
            assert!(decode_transaction(&invalid).is_err());
        }
    }
}

#[test]
fn malformed_list_offsets_are_rejected() {
    let bytes = encode_transactions(&four_variants()).unwrap();
    for offset in [0_u32, 3, 20, u32::MAX] {
        let mut invalid = bytes.clone();
        invalid[..4].copy_from_slice(&offset.to_le_bytes());
        assert!(decode_transactions(&invalid).is_err());
    }
    let first = bytes[..4].to_vec();
    for offset in [0_u32, 16, u32::MAX] {
        let mut invalid = bytes.clone();
        invalid[4..8].copy_from_slice(&offset.to_le_bytes());
        assert!(decode_transactions(&invalid).is_err());
    }
    let mut overlapping = bytes;
    overlapping[4..8].copy_from_slice(&first);
    assert!(decode_transactions(&overlapping).is_err());
}

#[test]
fn invalid_signature_scalars_are_rejected() {
    let order = (SECP256K1N_HALF << 1) + U256::from(1);
    for signature in [
        Signature::new(U256::ZERO, U256::from(1), false),
        Signature::new(order, U256::from(1), false),
        Signature::new(U256::from(1), U256::ZERO, false),
        Signature::new(U256::from(1), SECP256K1N_HALF + U256::from(1), false),
    ] {
        assert!(matches!(
            TransactionSsz::new(four_variants()[0].payload().clone(), signature),
            Err(TransactionError::InvalidSignature),
        ));
    }
}

#[test]
fn full_width_values_survive_ssz_without_truncating_into_alloy() {
    for field in ["chain_id", "gas_price"] {
        let TransactionPayload::Legacy(mut payload) = four_variants()[2].payload().clone() else {
            panic!("expected legacy payload");
        };
        if field == "chain_id" {
            payload.chain_id = Some(U256::MAX);
        } else {
            payload.gas_price = U256::MAX;
        }
        let transaction = TransactionSsz::new(payload, signature()).unwrap();
        let decoded = decode_transaction(&encode_transaction(&transaction).unwrap()).unwrap();
        assert_eq!(decoded, transaction);
        assert!(matches!(
            decoded.to_envelope(),
            Err(TransactionError::UnsupportedWidth(actual)) if actual == field,
        ));
    }
}

#[test]
fn source_conversion_supports_typed_and_rejects_incorrect_cached_hashes() {
    let EthereumTxEnvelope::Legacy(legacy) = four_variants()[0].to_envelope().unwrap() else {
        panic!("expected legacy envelope");
    };
    let invalid_hash = Signed::new_unchecked(legacy.tx().clone(), *legacy.signature(), B256::ZERO);
    assert!(matches!(
        TransactionSsz::from_envelope(&EthereumTxEnvelope::Legacy(invalid_hash)),
        Err(TransactionError::SourceHashMismatch),
    ));

    let transactions = vec![
        EthereumTxEnvelope::Legacy(legacy),
        EthereumTxEnvelope::Eip1559(Signed::new_unhashed(TxEip1559::default(), signature())),
    ];
    let converted = convert_transactions(&transactions).unwrap();
    assert_eq!(converted[1].selector(), 8);

    let mut rlp = four_variants()[0].original_rlp().unwrap().to_vec();
    rlp.push(0);
    assert!(matches!(TransactionSsz::from_rlp(&rlp), Err(TransactionError::TrailingBytes)));
}

fn reference_transactions() -> Vec<TransactionSsz> {
    VECTORS
        .iter()
        .map(|(_, rlp, _, _, _, _)| {
            TransactionSsz::from_rlp(&alloy_primitives::hex::decode(rlp).unwrap()).unwrap()
        })
        .collect()
}

#[test]
fn all_ten_selectors_match_independent_bytes_hashes_and_object_roots() {
    for (selector, rlp, ssz, signing_hash, transaction_hash, object_root) in VECTORS {
        let original = alloy_primitives::hex::decode(rlp).unwrap();
        let canonical = alloy_primitives::hex::decode(ssz).unwrap();
        let transaction = TransactionSsz::from_rlp(&original).unwrap();
        assert_eq!(transaction.selector(), *selector);
        assert_eq!(encode_transaction(&transaction).unwrap(), canonical);
        assert_eq!(decode_transaction(&canonical).unwrap(), transaction);
        assert_eq!(transaction.original_rlp().unwrap().as_ref(), original);
        assert_eq!(
            transaction.original_signing_hash().unwrap(),
            signing_hash.parse::<B256>().unwrap()
        );
        assert_eq!(
            transaction.original_transaction_hash().unwrap(),
            transaction_hash.parse::<B256>().unwrap()
        );
        let native = transaction.to_envelope().unwrap();
        assert_eq!(TransactionSsz::from_envelope(&native).unwrap(), transaction);
        let snapshot = TransactionSnapshot::build(vec![transaction]).unwrap();
        assert_eq!(
            snapshot.transaction_tree(0).unwrap().root(),
            object_root.parse::<B256>().unwrap()
        );
    }
}

#[test]
fn mixed_lists_match_independent_nonempty_roots_and_retained_elements() {
    let reference = reference_transactions();
    for (count, expected) in LIST_ROOTS {
        let transactions =
            (0..*count).map(|i| reference[i % reference.len()].clone()).collect::<Vec<_>>();
        let snapshot = TransactionSnapshot::build(transactions.clone()).unwrap();
        assert_eq!(snapshot.root(), expected.parse::<B256>().unwrap());
        let decoded = TransactionSnapshot::from_ssz(snapshot.serialized()).unwrap();
        assert_eq!(decoded.transactions(), transactions);
        assert_eq!(decoded.root(), snapshot.root());
        for i in 0..*count {
            assert_eq!(
                snapshot.transaction_tree(i).unwrap().root(),
                VECTORS[i % reference.len()].5.parse::<B256>().unwrap()
            );
        }
        assert!(snapshot.transaction_tree(*count).is_none());
    }
}

#[test]
fn wide_legacy_v_and_all_typed_fee_widths_preserve_signed_material() {
    for transaction in reference_transactions() {
        let mut payload = transaction.payload().clone();
        match &mut payload {
            TransactionPayload::Legacy(p) => p.chain_id = Some(U256::MAX),
            TransactionPayload::AccessList(p) => p.chain_id = U256::MAX,
            TransactionPayload::FeeMarket(p) => p.chain_id = U256::MAX,
            TransactionPayload::Blob(p) => p.chain_id = U256::MAX,
            TransactionPayload::SetCode(p) => p.chain_id = U256::MAX,
        }
        let wide = TransactionSsz::new(payload, *transaction.signature()).unwrap();
        assert_eq!(TransactionSsz::from_rlp(&wide.original_rlp().unwrap()).unwrap(), wide);
        assert_eq!(decode_transaction(&encode_transaction(&wide).unwrap()).unwrap(), wide);
        assert!(matches!(wide.to_envelope(), Err(TransactionError::UnsupportedWidth("chain_id"))));

        let mut payload = transaction.payload().clone();
        match &mut payload {
            TransactionPayload::Legacy(p) => p.gas_price = U256::MAX,
            TransactionPayload::AccessList(p) => p.gas_price = U256::MAX,
            TransactionPayload::FeeMarket(p) => p.max_fee_per_gas = U256::MAX,
            TransactionPayload::Blob(p) => p.max_fee_per_blob_gas = U256::MAX,
            TransactionPayload::SetCode(p) => p.max_priority_fee_per_gas = U256::MAX,
        }
        let wide = TransactionSsz::new(payload, *transaction.signature()).unwrap();
        assert_eq!(TransactionSsz::from_rlp(&wide.original_rlp().unwrap()).unwrap(), wide);
        assert_eq!(decode_transaction(&encode_transaction(&wide).unwrap()).unwrap(), wide);
        assert!(matches!(wide.to_envelope(), Err(TransactionError::UnsupportedWidth(_))));
    }
}

#[test]
fn raw_bad_authorizations_duplicates_and_full_width_chain_survive() {
    let source = reference_transactions().into_iter().find(|tx| tx.selector() == 10).unwrap();
    let TransactionPayload::SetCode(mut payload) = source.payload().clone() else { unreachable!() };
    let bad = TransactionAuthorization::new_unchecked(
        Authorization { chain_id: U256::ZERO, address: Address::ZERO, nonce: u64::MAX },
        255,
        U256::ZERO,
        U256::MAX,
    );
    let protected = TransactionAuthorization::new_unchecked(
        Authorization { chain_id: U256::MAX, ..bad.inner().clone() },
        bad.y_parity(),
        bad.r(),
        bad.s(),
    );
    payload.authorization_list = vec![bad.clone(), protected.clone(), bad.clone()];
    let transaction =
        TransactionSsz::new(TransactionPayload::SetCode(payload), *source.signature()).unwrap();
    assert_eq!(
        TransactionSsz::from_rlp(&transaction.original_rlp().unwrap()).unwrap(),
        transaction
    );
    assert_eq!(
        decode_transaction(&encode_transaction(&transaction).unwrap()).unwrap(),
        transaction
    );
    assert_eq!(
        TransactionSsz::from_envelope(&transaction.to_envelope().unwrap()).unwrap(),
        transaction
    );
    assert_eq!(codec::authorization_selector(&bad), 1);
    assert_eq!(codec::authorization_selector(&protected), 2);
    assert_eq!(protected.inner().chain_id(), &U256::MAX);
    assert_eq!(protected.y_parity(), 255);
    assert!(protected.signature().is_err());
}

#[test]
fn empty_fields_and_duplicate_access_entries_remain_canonical() {
    let source = reference_transactions().into_iter().find(|tx| tx.selector() == 5).unwrap();
    let TransactionPayload::AccessList(mut p) = source.payload().clone() else { unreachable!() };
    p.input = Bytes::new();
    p.access_list = AccessList(vec![
        AccessListItem { address: Address::ZERO, storage_keys: vec![B256::ZERO, B256::ZERO] },
        AccessListItem { address: Address::ZERO, storage_keys: Vec::new() },
    ]);
    let transaction =
        TransactionSsz::new(TransactionPayload::AccessList(p.clone()), *source.signature())
            .unwrap();
    assert_eq!(
        decode_transaction(&encode_transaction(&transaction).unwrap()).unwrap(),
        transaction
    );
    assert_eq!(
        TransactionSsz::from_envelope(&transaction.to_envelope().unwrap()).unwrap(),
        transaction
    );
    p.access_list = AccessList::default();
    let empty =
        TransactionSsz::new(TransactionPayload::AccessList(p), *source.signature()).unwrap();
    assert_eq!(decode_transaction(&encode_transaction(&empty).unwrap()).unwrap(), empty);
}

#[test]
fn source_errors_retain_index_and_typed_cause_without_partial_conversion() {
    let source = reference_transactions();
    let good = source[0].to_envelope().unwrap();
    let EthereumTxEnvelope::Legacy(legacy) = four_variants()[0].to_envelope().unwrap() else {
        panic!("expected legacy envelope");
    };
    let bad = EthereumTxEnvelope::Legacy(Signed::new_unchecked(
        legacy.tx().clone(),
        signature(),
        B256::ZERO,
    ));
    match convert_transactions(&[good.clone(), bad, good]) {
        Err(TransactionError::AtTransaction { index: 1, source }) => {
            assert!(matches!(*source, TransactionError::SourceHashMismatch));
        }
        other => panic!("unexpected conversion result: {other:?}"),
    }
}

#[test]
fn all_selector_decoders_reject_truncation_wrong_type_and_noncanonical_offsets() {
    for transaction in reference_transactions() {
        let canonical = encode_transaction(&transaction).unwrap();
        for end in 0..canonical.len() {
            assert!(decode_transaction(&canonical[..end]).is_err());
        }
        let mut wrong_type = canonical.clone();
        wrong_type[9] ^= 0x80;
        assert!(decode_transaction(&wrong_type).is_err());
        let mut wrong_offset = canonical.clone();
        wrong_offset[..4].copy_from_slice(&9_u32.to_le_bytes());
        assert!(decode_transaction(&wrong_offset).is_err());
        let mut trailing = canonical;
        trailing.push(0);
        assert!(decode_transaction(&trailing).is_err());
        let mut rlp = transaction.original_rlp().unwrap().to_vec();
        rlp.push(0);
        assert!(TransactionSsz::from_rlp(&rlp).is_err());
    }
}

#[test]
fn empty_setcode_authorizations_are_not_valid_transactions() {
    let source = reference_transactions().into_iter().find(|tx| tx.selector() == 10).unwrap();
    let TransactionPayload::SetCode(mut payload) = source.payload().clone() else { unreachable!() };
    payload.authorization_list.clear();
    assert!(matches!(
        TransactionSsz::new(TransactionPayload::SetCode(payload), *source.signature()),
        Err(TransactionError::InvalidEncoding("empty authorization list"))
    ));
}

#[test]
fn full_width_hashes_and_roots_match_independent_expectations() {
    let originals = reference_transactions();
    for (index, change_chain, signing_hash, transaction_hash, root) in WIDE_EXPECTATIONS {
        let original = &originals[*index];
        let mut payload = original.payload().clone();
        if *change_chain {
            match &mut payload {
                TransactionPayload::Legacy(p) => p.chain_id = Some(U256::MAX),
                TransactionPayload::AccessList(p) => p.chain_id = U256::MAX,
                TransactionPayload::FeeMarket(p) => p.chain_id = U256::MAX,
                TransactionPayload::Blob(p) => p.chain_id = U256::MAX,
                TransactionPayload::SetCode(p) => p.chain_id = U256::MAX,
            }
        } else {
            match &mut payload {
                TransactionPayload::Legacy(p) => p.gas_price = U256::MAX,
                TransactionPayload::AccessList(p) => p.gas_price = U256::MAX,
                TransactionPayload::FeeMarket(p) => p.max_fee_per_gas = U256::MAX,
                TransactionPayload::Blob(p) => p.max_fee_per_blob_gas = U256::MAX,
                TransactionPayload::SetCode(p) => p.max_priority_fee_per_gas = U256::MAX,
            }
        }
        let wide = TransactionSsz::new(payload, *original.signature()).unwrap();
        assert_eq!(wide.original_signing_hash().unwrap(), signing_hash.parse::<B256>().unwrap());
        assert_eq!(
            wide.original_transaction_hash().unwrap(),
            transaction_hash.parse::<B256>().unwrap()
        );
        let snapshot = TransactionSnapshot::build(vec![wide]).unwrap();
        assert_eq!(snapshot.transaction_tree(0).unwrap().root(), root.parse::<B256>().unwrap());
    }
}

#[test]
fn maximum_native_chain_and_fee_values_convert_without_narrowing() {
    for original in reference_transactions() {
        let mut payload = original.payload().clone();
        let chain = U256::from(u64::MAX);
        let fee = U256::from(u128::MAX);
        match &mut payload {
            TransactionPayload::Legacy(p) => {
                p.chain_id = Some(chain);
                p.gas_price = fee;
            }
            TransactionPayload::AccessList(p) => {
                p.chain_id = chain;
                p.gas_price = fee;
            }
            TransactionPayload::FeeMarket(p) => {
                p.chain_id = chain;
                p.max_fee_per_gas = fee;
                p.max_priority_fee_per_gas = fee;
            }
            TransactionPayload::Blob(p) => {
                p.chain_id = chain;
                p.max_fee_per_gas = fee;
                p.max_priority_fee_per_gas = fee;
                p.max_fee_per_blob_gas = fee;
            }
            TransactionPayload::SetCode(p) => {
                p.chain_id = chain;
                p.max_fee_per_gas = fee;
                p.max_priority_fee_per_gas = fee;
            }
        }
        let transaction = TransactionSsz::new(payload, *original.signature()).unwrap();
        assert_eq!(
            TransactionSsz::from_envelope(&transaction.to_envelope().unwrap()).unwrap(),
            transaction
        );
    }
}

#[test]
fn first_values_beyond_native_widths_are_rejected_by_field() {
    for original in reference_transactions() {
        let mut payload = original.payload().clone();
        let overflow = U256::from(u64::MAX) + U256::from(1);
        match &mut payload {
            TransactionPayload::Legacy(p) => p.chain_id = Some(overflow),
            TransactionPayload::AccessList(p) => p.chain_id = overflow,
            TransactionPayload::FeeMarket(p) => p.chain_id = overflow,
            TransactionPayload::Blob(p) => p.chain_id = overflow,
            TransactionPayload::SetCode(p) => p.chain_id = overflow,
        }
        let transaction = TransactionSsz::new(payload, *original.signature()).unwrap();
        assert!(matches!(
            transaction.to_envelope(),
            Err(TransactionError::UnsupportedWidth("chain_id"))
        ));
        let mut payload = original.payload().clone();
        let overflow = U256::from(u128::MAX) + U256::from(1);
        let expected = match &mut payload {
            TransactionPayload::Legacy(p) => {
                p.gas_price = overflow;
                "gas_price"
            }
            TransactionPayload::AccessList(p) => {
                p.gas_price = overflow;
                "gas_price"
            }
            TransactionPayload::FeeMarket(p) => {
                p.max_priority_fee_per_gas = overflow;
                "max_priority_fee_per_gas"
            }
            TransactionPayload::Blob(p) => {
                p.max_priority_fee_per_gas = overflow;
                "max_priority_fee_per_gas"
            }
            TransactionPayload::SetCode(p) => {
                p.max_fee_per_gas = overflow;
                "max_fee_per_gas"
            }
        };
        let transaction = TransactionSsz::new(payload, *original.signature()).unwrap();
        assert!(
            matches!(transaction.to_envelope(), Err(TransactionError::UnsupportedWidth(field)) if field == expected)
        );
    }
}

#[test]
fn arbitrary_and_mutated_inputs_never_accept_noncanonical_bytes() {
    let mut state = 0x51a2_93c4_u64;
    for length in 0..512 {
        let bytes = (0..length)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state >> 32) as u8
            })
            .collect::<Vec<_>>();
        if let Ok(transaction) = decode_transaction(&bytes) {
            assert_eq!(encode_transaction(&transaction).unwrap(), bytes);
        }
        if let Ok(transaction) = TransactionSsz::from_rlp(&bytes) {
            assert_eq!(transaction.original_rlp().unwrap().as_ref(), bytes);
        }
        let _ = decode_transactions(&bytes);
    }
    for original in reference_transactions() {
        let bytes = encode_transaction(&original).unwrap();
        for index in (0..bytes.len()).step_by(17) {
            let mut mutated = bytes.clone();
            mutated[index] ^= 0x80;
            if let Ok(transaction) = decode_transaction(&mutated) {
                assert_eq!(encode_transaction(&transaction).unwrap(), mutated);
            }
        }
    }
}

const VECTORS: &[(u8, &str, &str, &str, &str, &str)] = &[
    (1, "f86780862d79883d2000825208945df9b87991262f6ba471f09758cde1c0fc1de734827a69801ca088ff6cf0fefd94db46111149ae4bfc179e9b94721fffd821d38d16464b3f71d0a045e0aff800961cfce805daef7016b9b675c137a6a41a548f7b60a3484c06a33a", "08000000720000000100000000000000000000203d88792d000000000000000000000000000000000000000000000000000008520000000000005df9b87991262f6ba471f09758cde1c0fc1de734697a000000000000000000000000000000000000000000000000000000000000690000000088ff6cf0fefd94db46111149ae4bfc179e9b94721fffd821d38d16464b3f71d045e0aff800961cfce805daef7016b9b675c137a6a41a548f7b60a3484c06a33a01", "19b1e28c14f33e74b96b88eba97d4a4fc8a97638d72e972310025b7e1189b049", "5c504ed432cb51138bcf09aa5e8a410dd4a1e204ef84bfed1be16dfba1b22060", "bef7b49904562eac794ae36b9e675e46d638bc4121453f9a46bbe03473d1ab87"),
    (1, "f86e8243eb850df847580082c35094df190dc7190dfba737d7777a163445b7fff161338806113a84987be800801ca03b08715b4403c792b8c7567edea634088bedcd7f60d9352b1f16c69830f3afd5a010b9afb67d2ec8b956f0e1dbc07eb79152904f3a7bf789fc869db56320adfe09", "08000000720000000100eb43000000000000005847f80d00000000000000000000000000000000000000000000000000000050c3000000000000df190dc7190dfba737d7777a163445b7fff1613300e87b98843a110600000000000000000000000000000000000000000000000069000000003b08715b4403c792b8c7567edea634088bedcd7f60d9352b1f16c69830f3afd510b9afb67d2ec8b956f0e1dbc07eb79152904f3a7bf789fc869db56320adfe0901", "05887873a4fc88813466382673be81701adc8f564f7bcd17e2ce974a43100d04", "e9e91f1ee4b56c0df2e9f06c2b8c27c6076195a88a7b8537ba8313d80e6f124e", "b5fd697f31a68d85d6502a137b11548302dffa23b1954aa6b12d105599a82ad2"),
    (3, "f88b822ecd8509839089a083015f909484654be796dad370032391d5479f8f1fd9ddd14e80a4d508e62389272d541e4168e5303b5838dcef5f9ac769a057f8dc6aabce7ec1e69b7ce5e625a078e88f9c69d217c76d92d5472f5812501021342f0054178c0579ee532dc2a218a0484723f0933633a6c213fc201ee3aecbe919579377b11a79acd887950a068e33", "08000000b600000003000100000000000000000000000000000000000000000000000000000000000000cd2e000000000000a089908309000000000000000000000000000000000000000000000000000000905f01000000000084654be796dad370032391d5479f8f1fd9ddd14e000000000000000000000000000000000000000000000000000000000000000089000000d508e62389272d541e4168e5303b5838dcef5f9ac769a057f8dc6aabce7ec1e69b7ce5e60078e88f9c69d217c76d92d5472f5812501021342f0054178c0579ee532dc2a218484723f0933633a6c213fc201ee3aecbe919579377b11a79acd887950a068e3300", "3e4c94ee2d68abe91ab5c97ad1111f5367a8cbdc8e907850ff9a297a11755441", "dc81918bf78322ce017c592e81c855f40bb96bd82da9779167de1de109962be6", "ad2afaa33122dd0c7a4c18f600f18d256cfe503dab5b2e24e0029455109f89cd"),
    (5, "01f90cc30182153680830bb224943765521db364ee269e9540970fd21e5a3e82569980b905845f53273d00000000000000000000000000000000000000000000000000112cfa39a81d7c00000000000000000000000088e6a0c2ddd26feeb64f039a2c41296fcb3f5640000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000004e4128acb080000000000000000000000003765521db364ee269e9540970fd21e5a3e825699000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000001dac7be33000000000000000000000000000000000000000000000000000000010005bd8200000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000004200000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000010000000000000000000000006a9850e46518231b23e50467c975fa94026be5d5000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000324128acb0800000000000000000000000088e6a0c2ddd26feeb64f039a2c41296fcb3f564000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000001ade409ce94c21807e0000000000000000000000000000000000000000000000000000000010005bd8200000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000260000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000002000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2000000000000000000000000f1f85b2c54a2bd284b1cf4141d64fd171bd855390000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000c00000000000000000000000000000000000000000000000000000000000000044a9059cbb000000000000000000000000f1f85b2c54a2bd284b1cf4141d64fd171bd8553900000000000000000000000000000000000000000000000034a2e5498f6f66f80000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000a4022c0d9f0000000000000000000000000000000000000000000001ade409ce94c21807e000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006a9850e46518231b23e50467c975fa94026be5d500000000000000000000000000000000000000000000000000000000000000800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f906d7f859941c86b3cdf2a60ae3a574f7f71d44e2c50bddb87ef842a00000000000000000000000000000000000000000000000000000000000000003a050f2c0a7453ed72f1987f5c26e34dd531d584edf35a9b2dd71d8cbd0b46cbec2f87a94c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2f863a0c5192a5360898ecb25d40688e79d4568e38430536a3f38f85b3859328d3cedbca0cc805915145ea1d9dd6fe22fe624a946d6b02968a5cf3e32ab5376b3fcedd207a0390f6178407c9b8e95802b8659e6df8e34c1e3d4f8d6a49e6132bbcdd937b63af7943765521db364ee269e9540970fd21e5a3e825699e1a00000000000000000000000000000000000000000000000000000000000000000f8fe94a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48f8e7a010d6a54a4754c8869d6886b5f5d7fbfa5b4522237ea5c60d11bc4e7a1ff9390ba07050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c3a01f21a62c4538bacf2aabeca410f0fe63151869f172e03c0e00357ba26a341effa00000000000000000000000000000000000000000000000000000000000000001a0faa1053f6d4d1d5c438dc7bcb1d301a860f75f57f936878518deaed1f60940d0a0390f6178407c9b8e95802b8659e6df8e34c1e3d4f8d6a49e6132bbcdd937b63aa0441b08c38c1e5e3510dc0c6fdeb4e6a8d60744b906171767ae804339d85b866cf89b946a9850e46518231b23e50467c975fa94026be5d5f884a0918cfc45073e7a8befeda107cec8f3ce514f4a406e227c253b570e1d6485d524a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000004a00000000000000000000000000000000000000000000000000000000000000001f79457ab1ec28d129707052df4df418d58a2d46d5f51e1a00000000000000000000000000000000000000000000000000000000000000002f87a9405a9cbe762b36632b3594da4f082340e0e5343e8f863a0faa1053f6d4d1d5c438dc7bcb1d301a860f75f57f936878518deaed1f60940d0a0c5192a5360898ecb25d40688e79d4568e38430536a3f38f85b3859328d3cedbca00000000000000000000000000000000000000000000000000000000000000002f85994613c773c7a1d85d2f1dcc051b0573d33470762ebf842a00563a8a19823933e751ef690567f0351d13ee18500841743a77290b3deeac37da041feecc8dc55520132a464384eb4cad04fca8408899b069cf80a59042ac84c7af85994c757acba3c0506218b3022266a9dc7f3612d85f5f842a0ad77bf801726b2c83e9e3fa10f319bf2df99397a561126e8c3df0f7596519f75a0f0afdce5671e49e66c5829c89ac80a06875a30b8a123df04dc6cc02b9c7c845cf8dd94f1f85b2c54a2bd284b1cf4141d64fd171bd85539f8c6a00000000000000000000000000000000000000000000000000000000000000008a00000000000000000000000000000000000000000000000000000000000000006a00000000000000000000000000000000000000000000000000000000000000007a00000000000000000000000000000000000000000000000000000000000000009a0000000000000000000000000000000000000000000000000000000000000000aa0000000000000000000000000000000000000000000000000000000000000000cd6940000000000000000000000000000000000000000c0f89b9488e6a0c2ddd26feeb64f039a2c41296fcb3f5640f884a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000004a00000000000000000000000000000000000000000000000000000000000000001a0a6172f64e80168333d3bc97ef096a7cb251d24be47156dff7a40f6163a6ab42bd694a2327a938febf5fec13bacfb16ae10ecbc4cbdcfc0f8dd944d8dbd193d89b7b506be5dc9db75b91da00d6a1df8c6a00000000000000000000000000000000000000000000000000000000000000004a0b79e191a25a1c10448c48948add5400e67bdba9b68cb8e4ab5ccc4025afe7bf6a0000000000000000000000000000000000000000000000000000000000000000ba0d8f0dca3c14b74279ad90a75338db7cccdd5b2448369aeb32a94693a1c4cde0da00000000000000000000000000000000000000000000000000000000000000005a00000000000000000000000000000000000000000000000000000000000000002f794545973f28950f50fc6c7f52aab4ad214a27c0564e1a097176f14e7115c17104cb26360d789269f7e452d9b8ac6a3b8945c07081b33ac01a0dc6b057ac2591f402a243c3b81e429e750442d0a86f0fc112cf2eaaac26d4b8da03e5cace679d3104fae7f63ef0e64e7f4be5cfae64f9e03aebc0e19e5de0f29f8", "08000000fe0c0000050101000000000000000000000000000000000000000000000000000000000000003615000000000000000000000000000000000000000000000000000000000000000000000000000024b20b00000000003765521db364ee269e9540970fd21e5a3e82569900000000000000000000000000000000000000000000000000000000000000008d000000110600005f53273d00000000000000000000000000000000000000000000000000112cfa39a81d7c00000000000000000000000088e6a0c2ddd26feeb64f039a2c41296fcb3f5640000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000004e4128acb080000000000000000000000003765521db364ee269e9540970fd21e5a3e825699000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000001dac7be33000000000000000000000000000000000000000000000000000000010005bd8200000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000004200000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000010000000000000000000000006a9850e46518231b23e50467c975fa94026be5d5000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000324128acb0800000000000000000000000088e6a0c2ddd26feeb64f039a2c41296fcb3f564000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000001ade409ce94c21807e0000000000000000000000000000000000000000000000000000000010005bd8200000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000260000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000002000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2000000000000000000000000f1f85b2c54a2bd284b1cf4141d64fd171bd855390000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000c00000000000000000000000000000000000000000000000000000000000000044a9059cbb000000000000000000000000f1f85b2c54a2bd284b1cf4141d64fd171bd8553900000000000000000000000000000000000000000000000034a2e5498f6f66f80000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000a4022c0d9f0000000000000000000000000000000000000000000001ade409ce94c21807e000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006a9850e46518231b23e50467c975fa94026be5d5000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000003c000000940000000c010000440100003c020000d40200000c03000084030000dc030000340400000c05000024050000bc050000d4050000ac0600001c86b3cdf2a60ae3a574f7f71d44e2c50bddb87e18000000000000000000000000000000000000000000000000000000000000000000000350f2c0a7453ed72f1987f5c26e34dd531d584edf35a9b2dd71d8cbd0b46cbec2c02aaa39b223fe8d0a0e5c4f27ead9083c756cc218000000c5192a5360898ecb25d40688e79d4568e38430536a3f38f85b3859328d3cedbccc805915145ea1d9dd6fe22fe624a946d6b02968a5cf3e32ab5376b3fcedd207390f6178407c9b8e95802b8659e6df8e34c1e3d4f8d6a49e6132bbcdd937b63a3765521db364ee269e9540970fd21e5a3e825699180000000000000000000000000000000000000000000000000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb481800000010d6a54a4754c8869d6886b5f5d7fbfa5b4522237ea5c60d11bc4e7a1ff9390b7050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c31f21a62c4538bacf2aabeca410f0fe63151869f172e03c0e00357ba26a341eff0000000000000000000000000000000000000000000000000000000000000001faa1053f6d4d1d5c438dc7bcb1d301a860f75f57f936878518deaed1f60940d0390f6178407c9b8e95802b8659e6df8e34c1e3d4f8d6a49e6132bbcdd937b63a441b08c38c1e5e3510dc0c6fdeb4e6a8d60744b906171767ae804339d85b866c6a9850e46518231b23e50467c975fa94026be5d518000000918cfc45073e7a8befeda107cec8f3ce514f4a406e227c253b570e1d6485d52400000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000157ab1ec28d129707052df4df418d58a2d46d5f5118000000000000000000000000000000000000000000000000000000000000000000000205a9cbe762b36632b3594da4f082340e0e5343e818000000faa1053f6d4d1d5c438dc7bcb1d301a860f75f57f936878518deaed1f60940d0c5192a5360898ecb25d40688e79d4568e38430536a3f38f85b3859328d3cedbc0000000000000000000000000000000000000000000000000000000000000002613c773c7a1d85d2f1dcc051b0573d33470762eb180000000563a8a19823933e751ef690567f0351d13ee18500841743a77290b3deeac37d41feecc8dc55520132a464384eb4cad04fca8408899b069cf80a59042ac84c7ac757acba3c0506218b3022266a9dc7f3612d85f518000000ad77bf801726b2c83e9e3fa10f319bf2df99397a561126e8c3df0f7596519f75f0afdce5671e49e66c5829c89ac80a06875a30b8a123df04dc6cc02b9c7c845cf1f85b2c54a2bd284b1cf4141d64fd171bd85539180000000000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000600000000000000000000000000000000000000000000000000000000000000070000000000000000000000000000000000000000000000000000000000000009000000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000000c00000000000000000000000000000000000000001800000088e6a0c2ddd26feeb64f039a2c41296fcb3f564018000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000001a6172f64e80168333d3bc97ef096a7cb251d24be47156dff7a40f6163a6ab42ba2327a938febf5fec13bacfb16ae10ecbc4cbdcf180000004d8dbd193d89b7b506be5dc9db75b91da00d6a1d180000000000000000000000000000000000000000000000000000000000000000000004b79e191a25a1c10448c48948add5400e67bdba9b68cb8e4ab5ccc4025afe7bf6000000000000000000000000000000000000000000000000000000000000000bd8f0dca3c14b74279ad90a75338db7cccdd5b2448369aeb32a94693a1c4cde0d00000000000000000000000000000000000000000000000000000000000000050000000000000000000000000000000000000000000000000000000000000002545973f28950f50fc6c7f52aab4ad214a27c05641800000097176f14e7115c17104cb26360d789269f7e452d9b8ac6a3b8945c07081b33ac00dc6b057ac2591f402a243c3b81e429e750442d0a86f0fc112cf2eaaac26d4b8d3e5cace679d3104fae7f63ef0e64e7f4be5cfae64f9e03aebc0e19e5de0f29f801", "f4dbd7e72dc303d0790b23d56e88034bbf368e7f56a603ba345ccec74f454365", "8135b5403ea5528341d661cdadd8eb67983909fc1644b0a133de708b13e10937", "2cfca79a964f7a00191f1e688683c1b44e6f0990dfc3f503d4aca76a49aac270"),
    (7, "02f9034c01830f9b358085064a2d5a958302690c946b75d8af000000e20b7a7ddf000ba900b4009a808403e168d79b2b2f1a397217ae1de4a6e543858b0191c9213aadbcc4ed256de6ecf902c0f85994c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2f842a012231cd4c753cb5530a43a74c45106c24765e6f81dc8927d4f4be7e53315d5a8a0f3bba710d3566d618899e2756f47b6b6ab971f4e7a9b49c8676839ce70a3b615f8dd94397217ae1de4a6e543858b0191c9213aadbcc4edf8c6a0000000000000000000000000000000000000000000000000000000000000000ca00000000000000000000000000000000000000000000000000000000000000008a00000000000000000000000000000000000000000000000000000000000000006a00000000000000000000000000000000000000000000000000000000000000007a00000000000000000000000000000000000000000000000000000000000000009a0000000000000000000000000000000000000000000000000000000000000000af90183944ecfc56672c7630b84dac9c1f7407579715de155f9016ba0000000000000000000000000000000000000000000000000000000000000000fa038ed69781a961bb9622064044b9913b1581bddd02ed6468e0e8c3899ae4ff6cea029cb8bd4e192d16f51155329ce8b0f5eb88a1d9e4d3b93ce07efbac9e1c4d175a0a34640eb1c607d836ceb4af07c59befcec4cdb4943351c3a245f8712063aa3b6a0000000000000000000000000000000000000000000000000000000000000000aa00000000000000000000000000000000000000000000000000000000000000008a00e0b1ffc883156bcc50271d13adbf2142a3da019b453b028078482a0967df23ba0dc59cdc7b4ec63ea402af3d39d169ec1338c1dd00754c63deca3620ef11ce970a04eb8e6d369a44c627e68958caefca5b050389afd4410af55103da22160f32e6fa0e9c8ce3daf53241f9a1be37866116a52a57048d3c7f27faf9b4e8c6e2e50cf81a0489f94fa9c8ead25fac83530419e4f3ae96ec772b8179124ec3bf465ad8ddb2e01a0bcd30031cd0ee1132ec0fd4debd5157269adf0e478e4c4733d846b09f8891dcda02afb6af719e9b548c7d543e7698c5a9c15f97d7f126277d7178ff33670d84e08", "080000008503000007020100000000000000000000000000000000000000000000000000000000000000359b0f0000000000955a2d4a060000000000000000000000000000000000000000000000000000000c690200000000006b75d8af000000e20b7a7ddf000ba900b4009a80d768e10300000000000000000000000000000000000000000000000000000000ad000000c800000000000000000000000000000000000000000000000000000000000000000000002b2f1a397217ae1de4a6e543858b0191c9213aadbcc4ed256de6ec0c000000640000003c010000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc21800000012231cd4c753cb5530a43a74c45106c24765e6f81dc8927d4f4be7e53315d5a8f3bba710d3566d618899e2756f47b6b6ab971f4e7a9b49c8676839ce70a3b615397217ae1de4a6e543858b0191c9213aadbcc4ed18000000000000000000000000000000000000000000000000000000000000000000000c0000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000600000000000000000000000000000000000000000000000000000000000000070000000000000000000000000000000000000000000000000000000000000009000000000000000000000000000000000000000000000000000000000000000a4ecfc56672c7630b84dac9c1f7407579715de15518000000000000000000000000000000000000000000000000000000000000000000000f38ed69781a961bb9622064044b9913b1581bddd02ed6468e0e8c3899ae4ff6ce29cb8bd4e192d16f51155329ce8b0f5eb88a1d9e4d3b93ce07efbac9e1c4d175a34640eb1c607d836ceb4af07c59befcec4cdb4943351c3a245f8712063aa3b6000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000080e0b1ffc883156bcc50271d13adbf2142a3da019b453b028078482a0967df23bdc59cdc7b4ec63ea402af3d39d169ec1338c1dd00754c63deca3620ef11ce9704eb8e6d369a44c627e68958caefca5b050389afd4410af55103da22160f32e6fe9c8ce3daf53241f9a1be37866116a52a57048d3c7f27faf9b4e8c6e2e50cf81489f94fa9c8ead25fac83530419e4f3ae96ec772b8179124ec3bf465ad8ddb2e00bcd30031cd0ee1132ec0fd4debd5157269adf0e478e4c4733d846b09f8891dcd2afb6af719e9b548c7d543e7698c5a9c15f97d7f126277d7178ff33670d84e0801", "b7dfe5ff249ba41f9043c9eabd0003d5b045c252147372391d6f401b64f04dd2", "ba363483b992ef59342094ab98b8523ed6c055b5788e6726140d2badf27bf6d6", "5aa39673a2a6bd8c9b85dd3d16a2cc6d2cb26f514181d059e9b96f27bb12a643"),
    (8, "02f911d401038419fa53ad850c87eaa4da830efb7a8080b9117a608060405260405162000eda38038062000eda83398101604081905262000026916200049d565b828162000036828260006200004d565b50620000449050826200008a565b505050620005d0565b6200005883620000e5565b600082511180620000665750805b1562000085576200008383836200012760201b620001791760201c565b505b505050565b7f7e644d79422f17c01e4894b5f4f588d331ebfa28653d42ae832dc59e38c9798f620000b562000156565b604080516001600160a01b03928316815291841660208301520160405180910390a1620000e2816200018f565b50565b620000f08162000244565b6040516001600160a01b038216907fbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b90600090a250565b60606200014f838360405180606001604052806027815260200162000eb360279139620002f8565b9392505050565b60006200018060008051602062000e9383398151915260001b6200037760201b620001a51760201c565b546001600160a01b0316919050565b6001600160a01b038116620001fa5760405162461bcd60e51b815260206004820152602660248201527f455243313936373a206e65772061646d696e20697320746865207a65726f206160448201526564647265737360d01b60648201526084015b60405180910390fd5b806200022360008051602062000e9383398151915260001b6200037760201b620001a51760201c565b80546001600160a01b0319166001600160a01b039290921691909117905550565b6200025a816200037a60201b620001a81760201c565b620002be5760405162461bcd60e51b815260206004820152602d60248201527f455243313936373a206e657720696d706c656d656e746174696f6e206973206e60448201526c1bdd08184818dbdb9d1c9858dd609a1b6064820152608401620001f1565b80620002237f360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc60001b6200037760201b620001a51760201c565b6060600080856001600160a01b0316856040516200031791906200057d565b600060405180830381855af49150503d806000811462000354576040519150601f19603f3d011682016040523d82523d6000602084013e62000359565b606091505b5090925090506200036d8683838762000389565b9695505050505050565b90565b6001600160a01b03163b151590565b60608315620003fa578251620003f2576001600160a01b0385163b620003f25760405162461bcd60e51b815260206004820152601d60248201527f416464726573733a2063616c6c20746f206e6f6e2d636f6e74726163740000006044820152606401620001f1565b508162000406565b6200040683836200040e565b949350505050565b8151156200041f5781518083602001fd5b8060405162461bcd60e51b8152600401620001f191906200059b565b80516001600160a01b03811681146200045357600080fd5b919050565b634e487b7160e01b600052604160045260246000fd5b60005b838110156200048b57818101518382015260200162000471565b83811115620000835750506000910152565b600080600060608486031215620004b357600080fd5b620004be846200043b565b9250620004ce602085016200043b565b60408501519092506001600160401b0380821115620004ec57600080fd5b818601915086601f8301126200050157600080fd5b81518181111562000516576200051662000458565b604051601f8201601f19908116603f0116810190838211818310171562000541576200054162000458565b816040528281528960208487010111156200055b57600080fd5b6200056e8360208301602088016200046e565b80955050505050509250925092565b60008251620005918184602087016200046e565b9190910192915050565b6020815260008251806020840152620005bc8160408501602087016200046e565b601f01601f19169190910160400192915050565b6108b380620005e06000396000f3fe60806040523661001357610011610017565b005b6100115b61001f6101b7565b6001600160a01b0316336001600160a01b0316141561016f5760606001600160e01b031960003516631b2ce7f360e11b8114156100655761005e6101ea565b9150610167565b6001600160e01b0319811663278f794360e11b14156100865761005e610241565b6001600160e01b031981166308f2839760e41b14156100a75761005e610287565b6001600160e01b031981166303e1469160e61b14156100c85761005e6102b8565b6001600160e01b03198116635c60da1b60e01b14156100e95761005e6102f8565b60405162461bcd60e51b815260206004820152604260248201527f5472616e73706172656e745570677261646561626c6550726f78793a2061646d60448201527f696e2063616e6e6f742066616c6c6261636b20746f2070726f78792074617267606482015261195d60f21b608482015260a4015b60405180910390fd5b815160208301f35b61017761030c565b565b606061019e83836040518060600160405280602781526020016108576027913961031c565b9392505050565b90565b6001600160a01b03163b151590565b60007fb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d61035b546001600160a01b0316919050565b60606101f4610394565b600061020336600481846106a2565b81019061021091906106e8565b905061022d8160405180602001604052806000815250600061039f565b505060408051602081019091526000815290565b606060008061025336600481846106a2565b8101906102609190610719565b915091506102708282600161039f565b604051806020016040528060008152509250505090565b6060610291610394565b60006102a036600481846106a2565b8101906102ad91906106e8565b905061022d816103cb565b60606102c2610394565b60006102cc6101b7565b604080516001600160a01b03831660208201529192500160405160208183030381529060405291505090565b6060610302610394565b60006102cc610422565b610177610317610422565b610431565b6060600080856001600160a01b0316856040516103399190610807565b600060405180830381855af49150503d8060008114610374576040519150601f19603f3d011682016040523d82523d6000602084013e610379565b606091505b509150915061038a86838387610455565b9695505050505050565b341561017757600080fd5b6103a8836104d3565b6000825111806103b55750805b156103c6576103c48383610179565b505b505050565b7f7e644d79422f17c01e4894b5f4f588d331ebfa28653d42ae832dc59e38c9798f6103f46101b7565b604080516001600160a01b03928316815291841660208301520160405180910390a161041f81610513565b50565b600061042c6105bc565b905090565b3660008037600080366000845af43d6000803e808015610450573d6000f35b3d6000fd5b606083156104c15782516104ba576001600160a01b0385163b6104ba5760405162461bcd60e51b815260206004820152601d60248201527f416464726573733a2063616c6c20746f206e6f6e2d636f6e7472616374000000604482015260640161015e565b50816104cb565b6104cb83836105e4565b949350505050565b6104dc8161060e565b6040516001600160a01b038216907fbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b90600090a250565b6001600160a01b0381166105785760405162461bcd60e51b815260206004820152602660248201527f455243313936373a206e65772061646d696e20697320746865207a65726f206160448201526564647265737360d01b606482015260840161015e565b807fb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d61035b80546001600160a01b0319166001600160a01b039290921691909117905550565b60007f360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc6101db565b8151156105f45781518083602001fd5b8060405162461bcd60e51b815260040161015e9190610823565b6001600160a01b0381163b61067b5760405162461bcd60e51b815260206004820152602d60248201527f455243313936373a206e657720696d706c656d656e746174696f6e206973206e60448201526c1bdd08184818dbdb9d1c9858dd609a1b606482015260840161015e565b807f360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc61059b565b600080858511156106b257600080fd5b838611156106bf57600080fd5b5050820193919092039150565b80356001600160a01b03811681146106e357600080fd5b919050565b6000602082840312156106fa57600080fd5b61019e826106cc565b634e487b7160e01b600052604160045260246000fd5b6000806040838503121561072c57600080fd5b610735836106cc565b9150602083013567ffffffffffffffff8082111561075257600080fd5b818501915085601f83011261076657600080fd5b81358181111561077857610778610703565b604051601f8201601f19908116603f011681019083821181831017156107a0576107a0610703565b816040528281528860208487010111156107b957600080fd5b8260208601602083013760006020848301015280955050505050509250929050565b60005b838110156107f65781810151838201526020016107de565b838111156103c45750506000910152565b600082516108198184602087016107db565b9190910192915050565b60208152600082518060208401526108428160408501602087016107db565b601f01601f1916919091016040019291505056fe416464726573733a206c6f772d6c6576656c2064656c65676174652063616c6c206661696c6564a264697066735822122012bb4f564f73959a03513dc74fc3c6e40e8386e6f02c16b78d6db00ce0aa16af64736f6c63430008090033b53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103416464726573733a206c6f772d6c6576656c2064656c65676174652063616c6c206661696c65640000000000000000000000007e22fcb742572515d1c3fef972ec066c995820ef000000000000000000000000d89f6892a2b58d50e0e8f51a961b12a25ce3f7570000000000000000000000000000000000000000000000000000000000000060000000000000000000000000000000000000000000000000000000000000020457405d050000000000000000000000005f4ec3df9cbd43714fe2740f5e3616155c5b8419000000000000000000000000dac17f958d2ee523a2206206994597c13d831ec70000000000000000000000000000000000000000000000000000000064c7eb930000000000000000000000000000000000000000000000000000000064d2bde300000000000000000000000000000000000000000000000000000000000000e0000000000000000000000000000000000000000000000000000000003b9aca000000000000000000000000004580900be782a567ad407cc8fcde24dbda2a1aae000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000000e000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000165a0bc00000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000009795118749000000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000023a8f4b6a00000000000000000000000000000000000000000000000000000000c080a06fd00841d705c002a846de1b56d0c36941cd2897a7bb19419d427a252bc4fff4a05be5ed4725f8b6047b5309da5f47d1e98fa2c79e5c0841595291fae7167de108", "080000001c120000080201000000000000000000000000000000000000000000000000000000000000000300000000000000daa4ea870c0000000000000000000000000000000000000000000000000000007afb0e000000000000000000000000000000000000000000000000000000000000000000000000009900000013120000ad53fa1900000000000000000000000000000000000000000000000000000000608060405260405162000eda38038062000eda83398101604081905262000026916200049d565b828162000036828260006200004d565b50620000449050826200008a565b505050620005d0565b6200005883620000e5565b600082511180620000665750805b1562000085576200008383836200012760201b620001791760201c565b505b505050565b7f7e644d79422f17c01e4894b5f4f588d331ebfa28653d42ae832dc59e38c9798f620000b562000156565b604080516001600160a01b03928316815291841660208301520160405180910390a1620000e2816200018f565b50565b620000f08162000244565b6040516001600160a01b038216907fbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b90600090a250565b60606200014f838360405180606001604052806027815260200162000eb360279139620002f8565b9392505050565b60006200018060008051602062000e9383398151915260001b6200037760201b620001a51760201c565b546001600160a01b0316919050565b6001600160a01b038116620001fa5760405162461bcd60e51b815260206004820152602660248201527f455243313936373a206e65772061646d696e20697320746865207a65726f206160448201526564647265737360d01b60648201526084015b60405180910390fd5b806200022360008051602062000e9383398151915260001b6200037760201b620001a51760201c565b80546001600160a01b0319166001600160a01b039290921691909117905550565b6200025a816200037a60201b620001a81760201c565b620002be5760405162461bcd60e51b815260206004820152602d60248201527f455243313936373a206e657720696d706c656d656e746174696f6e206973206e60448201526c1bdd08184818dbdb9d1c9858dd609a1b6064820152608401620001f1565b80620002237f360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc60001b6200037760201b620001a51760201c565b6060600080856001600160a01b0316856040516200031791906200057d565b600060405180830381855af49150503d806000811462000354576040519150601f19603f3d011682016040523d82523d6000602084013e62000359565b606091505b5090925090506200036d8683838762000389565b9695505050505050565b90565b6001600160a01b03163b151590565b60608315620003fa578251620003f2576001600160a01b0385163b620003f25760405162461bcd60e51b815260206004820152601d60248201527f416464726573733a2063616c6c20746f206e6f6e2d636f6e74726163740000006044820152606401620001f1565b508162000406565b6200040683836200040e565b949350505050565b8151156200041f5781518083602001fd5b8060405162461bcd60e51b8152600401620001f191906200059b565b80516001600160a01b03811681146200045357600080fd5b919050565b634e487b7160e01b600052604160045260246000fd5b60005b838110156200048b57818101518382015260200162000471565b83811115620000835750506000910152565b600080600060608486031215620004b357600080fd5b620004be846200043b565b9250620004ce602085016200043b565b60408501519092506001600160401b0380821115620004ec57600080fd5b818601915086601f8301126200050157600080fd5b81518181111562000516576200051662000458565b604051601f8201601f19908116603f0116810190838211818310171562000541576200054162000458565b816040528281528960208487010111156200055b57600080fd5b6200056e8360208301602088016200046e565b80955050505050509250925092565b60008251620005918184602087016200046e565b9190910192915050565b6020815260008251806020840152620005bc8160408501602087016200046e565b601f01601f19169190910160400192915050565b6108b380620005e06000396000f3fe60806040523661001357610011610017565b005b6100115b61001f6101b7565b6001600160a01b0316336001600160a01b0316141561016f5760606001600160e01b031960003516631b2ce7f360e11b8114156100655761005e6101ea565b9150610167565b6001600160e01b0319811663278f794360e11b14156100865761005e610241565b6001600160e01b031981166308f2839760e41b14156100a75761005e610287565b6001600160e01b031981166303e1469160e61b14156100c85761005e6102b8565b6001600160e01b03198116635c60da1b60e01b14156100e95761005e6102f8565b60405162461bcd60e51b815260206004820152604260248201527f5472616e73706172656e745570677261646561626c6550726f78793a2061646d60448201527f696e2063616e6e6f742066616c6c6261636b20746f2070726f78792074617267606482015261195d60f21b608482015260a4015b60405180910390fd5b815160208301f35b61017761030c565b565b606061019e83836040518060600160405280602781526020016108576027913961031c565b9392505050565b90565b6001600160a01b03163b151590565b60007fb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d61035b546001600160a01b0316919050565b60606101f4610394565b600061020336600481846106a2565b81019061021091906106e8565b905061022d8160405180602001604052806000815250600061039f565b505060408051602081019091526000815290565b606060008061025336600481846106a2565b8101906102609190610719565b915091506102708282600161039f565b604051806020016040528060008152509250505090565b6060610291610394565b60006102a036600481846106a2565b8101906102ad91906106e8565b905061022d816103cb565b60606102c2610394565b60006102cc6101b7565b604080516001600160a01b03831660208201529192500160405160208183030381529060405291505090565b6060610302610394565b60006102cc610422565b610177610317610422565b610431565b6060600080856001600160a01b0316856040516103399190610807565b600060405180830381855af49150503d8060008114610374576040519150601f19603f3d011682016040523d82523d6000602084013e610379565b606091505b509150915061038a86838387610455565b9695505050505050565b341561017757600080fd5b6103a8836104d3565b6000825111806103b55750805b156103c6576103c48383610179565b505b505050565b7f7e644d79422f17c01e4894b5f4f588d331ebfa28653d42ae832dc59e38c9798f6103f46101b7565b604080516001600160a01b03928316815291841660208301520160405180910390a161041f81610513565b50565b600061042c6105bc565b905090565b3660008037600080366000845af43d6000803e808015610450573d6000f35b3d6000fd5b606083156104c15782516104ba576001600160a01b0385163b6104ba5760405162461bcd60e51b815260206004820152601d60248201527f416464726573733a2063616c6c20746f206e6f6e2d636f6e7472616374000000604482015260640161015e565b50816104cb565b6104cb83836105e4565b949350505050565b6104dc8161060e565b6040516001600160a01b038216907fbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b90600090a250565b6001600160a01b0381166105785760405162461bcd60e51b815260206004820152602660248201527f455243313936373a206e65772061646d696e20697320746865207a65726f206160448201526564647265737360d01b606482015260840161015e565b807fb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d61035b80546001600160a01b0319166001600160a01b039290921691909117905550565b60007f360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc6101db565b8151156105f45781518083602001fd5b8060405162461bcd60e51b815260040161015e9190610823565b6001600160a01b0381163b61067b5760405162461bcd60e51b815260206004820152602d60248201527f455243313936373a206e657720696d706c656d656e746174696f6e206973206e60448201526c1bdd08184818dbdb9d1c9858dd609a1b606482015260840161015e565b807f360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc61059b565b600080858511156106b257600080fd5b838611156106bf57600080fd5b5050820193919092039150565b80356001600160a01b03811681146106e357600080fd5b919050565b6000602082840312156106fa57600080fd5b61019e826106cc565b634e487b7160e01b600052604160045260246000fd5b6000806040838503121561072c57600080fd5b610735836106cc565b9150602083013567ffffffffffffffff8082111561075257600080fd5b818501915085601f83011261076657600080fd5b81358181111561077857610778610703565b604051601f8201601f19908116603f011681019083821181831017156107a0576107a0610703565b816040528281528860208487010111156107b957600080fd5b8260208601602083013760006020848301015280955050505050509250929050565b60005b838110156107f65781810151838201526020016107de565b838111156103c45750506000910152565b600082516108198184602087016107db565b9190910192915050565b60208152600082518060208401526108428160408501602087016107db565b601f01601f1916919091016040019291505056fe416464726573733a206c6f772d6c6576656c2064656c65676174652063616c6c206661696c6564a264697066735822122012bb4f564f73959a03513dc74fc3c6e40e8386e6f02c16b78d6db00ce0aa16af64736f6c63430008090033b53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103416464726573733a206c6f772d6c6576656c2064656c65676174652063616c6c206661696c65640000000000000000000000007e22fcb742572515d1c3fef972ec066c995820ef000000000000000000000000d89f6892a2b58d50e0e8f51a961b12a25ce3f7570000000000000000000000000000000000000000000000000000000000000060000000000000000000000000000000000000000000000000000000000000020457405d050000000000000000000000005f4ec3df9cbd43714fe2740f5e3616155c5b8419000000000000000000000000dac17f958d2ee523a2206206994597c13d831ec70000000000000000000000000000000000000000000000000000000064c7eb930000000000000000000000000000000000000000000000000000000064d2bde300000000000000000000000000000000000000000000000000000000000000e0000000000000000000000000000000000000000000000000000000003b9aca000000000000000000000000004580900be782a567ad407cc8fcde24dbda2a1aae000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000000e000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000165a0bc00000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000009795118749000000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000023a8f4b6a00000000000000000000000000000000000000000000000000000000006fd00841d705c002a846de1b56d0c36941cd2897a7bb19419d427a252bc4fff45be5ed4725f8b6047b5309da5f47d1e98fa2c79e5c0841595291fae7167de10800", "ef9132fa5b840358cf384376c697bf04ea540010d592ce683e9fde205bedf6fe", "a3b805acacb25da412a44bd9612a73464292fc684a400ab54f0a9626f7d9c3f2", "1baf88767daa723d3dff524616b5eec9b861c4bdd4ca2028e0185d718a4a4def"),
    (9, "03f8930180843b9aca008601d1e4e4ea008252089400000000000000000000000000000000000000008080c0843b9aca00e1a00175f564b393e44640ecffddce4010a42c1c966413987d8a59a253ef64a0c5cf01a04169d4bf9df18182d86715d4ec031c20aded1a4a2613f57e0c0169e16cf763c2a006fc81d18602b8a1e065193488a7c943dfd780adb7b72dfc332de7d68605986e", "08000000fa00000009030100000000000000000000000000000000000000000000000000000000000000000000000000000000eae4e4d101000000000000000000000000000000000000000000000000000000ca9a3b00000000000000000000000000000000000000000000000000000000085200000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000d1000000d100000000ca9a3b00000000000000000000000000000000000000000000000000000000d10000000175f564b393e44640ecffddce4010a42c1c966413987d8a59a253ef64a0c5cf004169d4bf9df18182d86715d4ec031c20aded1a4a2613f57e0c0169e16cf763c206fc81d18602b8a1e065193488a7c943dfd780adb7b72dfc332de7d68605986e01", "a872811828b0780fa9418b67ad12a6328dd88d4d378204955dbb6be0306490e1", "9c1fbda4f649ac806ab0faefbe94e1a60282eb374ead6aa01bac042f52b28a8c", "7e92a01094607312dacf16141c4a6d36e03e2090aaa2a6b86263079f4edbc9f6"),
    (10, "04f9017a010203040594000102030405060708091011121314151617181906824269f85bf859942021222324252627282930313233343536373839f842a0000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1fa0202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3ff8baf85a809430313233343536373839404142434445464748498001a0a4be86c16c6d3a2b907660b24187d0b30b69f6db3e6e8e7a7bb1183a4706d454a028aba84cdee6059dde41620422959d01da4f6cfff21a9b97036db018f1d815f6f85c0194505152535455565758596061626364656667686982030901a0a4be86c16c6d3a2b907660b24187d0b30b69f6db3e6e8e7a7bb1183a4706d454a028aba84cdee6059dde41620422959d01da4f6cfff21a9b97036db018f1d815f601a0a4be86c16c6d3a2b907660b24187d0b30b69f6db3e6e8e7a7bb1183a4706d454a028aba84cdee6059dde41620422959d01da4f6cfff21a9b97036db018f1d815f6", "08000000100200000a04010000000000000000000000000000000000000000000000000000000000000002000000000000000400000000000000000000000000000000000000000000000000000000000000050000000000000000010203040506070809101112131415161718190600000000000000000000000000000000000000000000000000000000000000b1000000b300000003000000000000000000000000000000000000000000000000000000000000000f010000426904000000202122232425262728293031323334353637383918000000000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f0800000070000000080000002600000001053031323334353637383940414243444546474849000000000000000000a4be86c16c6d3a2b907660b24187d0b30b69f6db3e6e8e7a7bb1183a4706d45428aba84cdee6059dde41620422959d01da4f6cfff21a9b97036db018f1d815f6010800000046000000020501000000000000000000000000000000000000000000000000000000000000005051525354555657585960616263646566676869090300000000000000a4be86c16c6d3a2b907660b24187d0b30b69f6db3e6e8e7a7bb1183a4706d45428aba84cdee6059dde41620422959d01da4f6cfff21a9b97036db018f1d815f60100a4be86c16c6d3a2b907660b24187d0b30b69f6db3e6e8e7a7bb1183a4706d45428aba84cdee6059dde41620422959d01da4f6cfff21a9b97036db018f1d815f601", "b1bf9454901a644d9a636f65f5cdcc04dc19901377c72c9fffec3f97c0709762", "aef87a34a0efb40e90614a96fa0dfa74c596dfb772ce8c73d624121f17aaa610", "89f1725dfa3f219e092af87023b7c862ebc8f76194e856c7971446ece51304a7"),
    (2, "f853098504a817c80082cf088007830102031ca088ff6cf0fefd94db46111149ae4bfc179e9b94721fffd821d38d16464b3f71d0a045e0aff800961cfce805daef7016b9b675c137a6a41a548f7b60a3484c06a33a", "08000000610000000200090000000000000000c817a80400000000000000000000000000000000000000000000000000000008cf0000000000000700000000000000000000000000000000000000000000000000000000000000550000000102030088ff6cf0fefd94db46111149ae4bfc179e9b94721fffd821d38d16464b3f71d045e0aff800961cfce805daef7016b9b675c137a6a41a548f7b60a3484c06a33a01", "569012af7df40e55b416c262bf2942f41bf36f984ecaea45491464cc71fa6927", "0fe128a23df1db30f68689f6743c59bea9a71b001414e2e3020eea31d4f3cb17", "cfdbf09099661b05b05b46dd65560055c7d1290d5f98252748661ad21906d47a"),
    (4, "f853098504a817c80082cf0880078301020326a088ff6cf0fefd94db46111149ae4bfc179e9b94721fffd821d38d16464b3f71d0a045e0aff800961cfce805daef7016b9b675c137a6a41a548f7b60a3484c06a33a", "080000008100000004000100000000000000000000000000000000000000000000000000000000000000090000000000000000c817a80400000000000000000000000000000000000000000000000000000008cf0000000000000700000000000000000000000000000000000000000000000000000000000000750000000102030088ff6cf0fefd94db46111149ae4bfc179e9b94721fffd821d38d16464b3f71d045e0aff800961cfce805daef7016b9b675c137a6a41a548f7b60a3484c06a33a01", "382e20ed386939bd56cba2f32016153bf80d7ed94734e34d375e999c0d310dd1", "7b7edaccbee2ea2bec813115a50a69c1d07730584c2aed7e025607c014c16b1e", "cc0652f06794faa3f6c4959bcb9945886ebea807f8fc0c55aa47086d03c32ae4"),
    (6, "01f90caf0182153680830bb2248080b905845f53273d00000000000000000000000000000000000000000000000000112cfa39a81d7c00000000000000000000000088e6a0c2ddd26feeb64f039a2c41296fcb3f5640000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000004e4128acb080000000000000000000000003765521db364ee269e9540970fd21e5a3e825699000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000001dac7be33000000000000000000000000000000000000000000000000000000010005bd8200000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000004200000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000010000000000000000000000006a9850e46518231b23e50467c975fa94026be5d5000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000324128acb0800000000000000000000000088e6a0c2ddd26feeb64f039a2c41296fcb3f564000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000001ade409ce94c21807e0000000000000000000000000000000000000000000000000000000010005bd8200000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000260000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000002000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2000000000000000000000000f1f85b2c54a2bd284b1cf4141d64fd171bd855390000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000c00000000000000000000000000000000000000000000000000000000000000044a9059cbb000000000000000000000000f1f85b2c54a2bd284b1cf4141d64fd171bd8553900000000000000000000000000000000000000000000000034a2e5498f6f66f80000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000a4022c0d9f0000000000000000000000000000000000000000000001ade409ce94c21807e000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006a9850e46518231b23e50467c975fa94026be5d500000000000000000000000000000000000000000000000000000000000000800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f906d7f859941c86b3cdf2a60ae3a574f7f71d44e2c50bddb87ef842a00000000000000000000000000000000000000000000000000000000000000003a050f2c0a7453ed72f1987f5c26e34dd531d584edf35a9b2dd71d8cbd0b46cbec2f87a94c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2f863a0c5192a5360898ecb25d40688e79d4568e38430536a3f38f85b3859328d3cedbca0cc805915145ea1d9dd6fe22fe624a946d6b02968a5cf3e32ab5376b3fcedd207a0390f6178407c9b8e95802b8659e6df8e34c1e3d4f8d6a49e6132bbcdd937b63af7943765521db364ee269e9540970fd21e5a3e825699e1a00000000000000000000000000000000000000000000000000000000000000000f8fe94a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48f8e7a010d6a54a4754c8869d6886b5f5d7fbfa5b4522237ea5c60d11bc4e7a1ff9390ba07050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c3a01f21a62c4538bacf2aabeca410f0fe63151869f172e03c0e00357ba26a341effa00000000000000000000000000000000000000000000000000000000000000001a0faa1053f6d4d1d5c438dc7bcb1d301a860f75f57f936878518deaed1f60940d0a0390f6178407c9b8e95802b8659e6df8e34c1e3d4f8d6a49e6132bbcdd937b63aa0441b08c38c1e5e3510dc0c6fdeb4e6a8d60744b906171767ae804339d85b866cf89b946a9850e46518231b23e50467c975fa94026be5d5f884a0918cfc45073e7a8befeda107cec8f3ce514f4a406e227c253b570e1d6485d524a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000004a00000000000000000000000000000000000000000000000000000000000000001f79457ab1ec28d129707052df4df418d58a2d46d5f51e1a00000000000000000000000000000000000000000000000000000000000000002f87a9405a9cbe762b36632b3594da4f082340e0e5343e8f863a0faa1053f6d4d1d5c438dc7bcb1d301a860f75f57f936878518deaed1f60940d0a0c5192a5360898ecb25d40688e79d4568e38430536a3f38f85b3859328d3cedbca00000000000000000000000000000000000000000000000000000000000000002f85994613c773c7a1d85d2f1dcc051b0573d33470762ebf842a00563a8a19823933e751ef690567f0351d13ee18500841743a77290b3deeac37da041feecc8dc55520132a464384eb4cad04fca8408899b069cf80a59042ac84c7af85994c757acba3c0506218b3022266a9dc7f3612d85f5f842a0ad77bf801726b2c83e9e3fa10f319bf2df99397a561126e8c3df0f7596519f75a0f0afdce5671e49e66c5829c89ac80a06875a30b8a123df04dc6cc02b9c7c845cf8dd94f1f85b2c54a2bd284b1cf4141d64fd171bd85539f8c6a00000000000000000000000000000000000000000000000000000000000000008a00000000000000000000000000000000000000000000000000000000000000006a00000000000000000000000000000000000000000000000000000000000000007a00000000000000000000000000000000000000000000000000000000000000009a0000000000000000000000000000000000000000000000000000000000000000aa0000000000000000000000000000000000000000000000000000000000000000cd6940000000000000000000000000000000000000000c0f89b9488e6a0c2ddd26feeb64f039a2c41296fcb3f5640f884a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000004a00000000000000000000000000000000000000000000000000000000000000001a0a6172f64e80168333d3bc97ef096a7cb251d24be47156dff7a40f6163a6ab42bd694a2327a938febf5fec13bacfb16ae10ecbc4cbdcfc0f8dd944d8dbd193d89b7b506be5dc9db75b91da00d6a1df8c6a00000000000000000000000000000000000000000000000000000000000000004a0b79e191a25a1c10448c48948add5400e67bdba9b68cb8e4ab5ccc4025afe7bf6a0000000000000000000000000000000000000000000000000000000000000000ba0d8f0dca3c14b74279ad90a75338db7cccdd5b2448369aeb32a94693a1c4cde0da00000000000000000000000000000000000000000000000000000000000000005a00000000000000000000000000000000000000000000000000000000000000002f794545973f28950f50fc6c7f52aab4ad214a27c0564e1a097176f14e7115c17104cb26360d789269f7e452d9b8ac6a3b8945c07081b33ac01a0dc6b057ac2591f402a243c3b81e429e750442d0a86f0fc112cf2eaaac26d4b8da03e5cace679d3104fae7f63ef0e64e7f4be5cfae64f9e03aebc0e19e5de0f29f8", "08000000ea0c0000060101000000000000000000000000000000000000000000000000000000000000003615000000000000000000000000000000000000000000000000000000000000000000000000000024b20b0000000000000000000000000000000000000000000000000000000000000000000000000079000000fd0500005f53273d00000000000000000000000000000000000000000000000000112cfa39a81d7c00000000000000000000000088e6a0c2ddd26feeb64f039a2c41296fcb3f5640000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000004e4128acb080000000000000000000000003765521db364ee269e9540970fd21e5a3e825699000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000001dac7be33000000000000000000000000000000000000000000000000000000010005bd8200000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000004200000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000010000000000000000000000006a9850e46518231b23e50467c975fa94026be5d5000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000324128acb0800000000000000000000000088e6a0c2ddd26feeb64f039a2c41296fcb3f564000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000001ade409ce94c21807e0000000000000000000000000000000000000000000000000000000010005bd8200000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000260000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000002000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2000000000000000000000000f1f85b2c54a2bd284b1cf4141d64fd171bd855390000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000c00000000000000000000000000000000000000000000000000000000000000044a9059cbb000000000000000000000000f1f85b2c54a2bd284b1cf4141d64fd171bd8553900000000000000000000000000000000000000000000000034a2e5498f6f66f80000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000a4022c0d9f0000000000000000000000000000000000000000000001ade409ce94c21807e000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006a9850e46518231b23e50467c975fa94026be5d5000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000003c000000940000000c010000440100003c020000d40200000c03000084030000dc030000340400000c05000024050000bc050000d4050000ac0600001c86b3cdf2a60ae3a574f7f71d44e2c50bddb87e18000000000000000000000000000000000000000000000000000000000000000000000350f2c0a7453ed72f1987f5c26e34dd531d584edf35a9b2dd71d8cbd0b46cbec2c02aaa39b223fe8d0a0e5c4f27ead9083c756cc218000000c5192a5360898ecb25d40688e79d4568e38430536a3f38f85b3859328d3cedbccc805915145ea1d9dd6fe22fe624a946d6b02968a5cf3e32ab5376b3fcedd207390f6178407c9b8e95802b8659e6df8e34c1e3d4f8d6a49e6132bbcdd937b63a3765521db364ee269e9540970fd21e5a3e825699180000000000000000000000000000000000000000000000000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb481800000010d6a54a4754c8869d6886b5f5d7fbfa5b4522237ea5c60d11bc4e7a1ff9390b7050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c31f21a62c4538bacf2aabeca410f0fe63151869f172e03c0e00357ba26a341eff0000000000000000000000000000000000000000000000000000000000000001faa1053f6d4d1d5c438dc7bcb1d301a860f75f57f936878518deaed1f60940d0390f6178407c9b8e95802b8659e6df8e34c1e3d4f8d6a49e6132bbcdd937b63a441b08c38c1e5e3510dc0c6fdeb4e6a8d60744b906171767ae804339d85b866c6a9850e46518231b23e50467c975fa94026be5d518000000918cfc45073e7a8befeda107cec8f3ce514f4a406e227c253b570e1d6485d52400000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000157ab1ec28d129707052df4df418d58a2d46d5f5118000000000000000000000000000000000000000000000000000000000000000000000205a9cbe762b36632b3594da4f082340e0e5343e818000000faa1053f6d4d1d5c438dc7bcb1d301a860f75f57f936878518deaed1f60940d0c5192a5360898ecb25d40688e79d4568e38430536a3f38f85b3859328d3cedbc0000000000000000000000000000000000000000000000000000000000000002613c773c7a1d85d2f1dcc051b0573d33470762eb180000000563a8a19823933e751ef690567f0351d13ee18500841743a77290b3deeac37d41feecc8dc55520132a464384eb4cad04fca8408899b069cf80a59042ac84c7ac757acba3c0506218b3022266a9dc7f3612d85f518000000ad77bf801726b2c83e9e3fa10f319bf2df99397a561126e8c3df0f7596519f75f0afdce5671e49e66c5829c89ac80a06875a30b8a123df04dc6cc02b9c7c845cf1f85b2c54a2bd284b1cf4141d64fd171bd85539180000000000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000600000000000000000000000000000000000000000000000000000000000000070000000000000000000000000000000000000000000000000000000000000009000000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000000c00000000000000000000000000000000000000001800000088e6a0c2ddd26feeb64f039a2c41296fcb3f564018000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000001a6172f64e80168333d3bc97ef096a7cb251d24be47156dff7a40f6163a6ab42ba2327a938febf5fec13bacfb16ae10ecbc4cbdcf180000004d8dbd193d89b7b506be5dc9db75b91da00d6a1d180000000000000000000000000000000000000000000000000000000000000000000004b79e191a25a1c10448c48948add5400e67bdba9b68cb8e4ab5ccc4025afe7bf6000000000000000000000000000000000000000000000000000000000000000bd8f0dca3c14b74279ad90a75338db7cccdd5b2448369aeb32a94693a1c4cde0d00000000000000000000000000000000000000000000000000000000000000050000000000000000000000000000000000000000000000000000000000000002545973f28950f50fc6c7f52aab4ad214a27c05641800000097176f14e7115c17104cb26360d789269f7e452d9b8ac6a3b8945c07081b33ac00dc6b057ac2591f402a243c3b81e429e750442d0a86f0fc112cf2eaaac26d4b8d3e5cace679d3104fae7f63ef0e64e7f4be5cfae64f9e03aebc0e19e5de0f29f801", "2b85fc76edac45d2e1b9e8cc560a671741bf401cecd374a38667472e61642b82", "78e5369e6bbfe52d381e14868201ac95b4e119a664333e804e13d2c6b6c27e52", "6e34138190d20535e5e014459d64eb7fa6c21cd9b0145be7ee6aec1930705c76"),
];
const LIST_ROOTS: &[(usize, &str)] = &[
    (0, "f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b"),
    (1, "2b8dd382421f976cab7b7a260d93942b712f242f0719eb92246eb5af076a98d5"),
    (4, "64e64cdcc69f1f304f7fbb9a36e4d0bfc14f29ae7027fcc588b292cca0a662e7"),
    (5, "a0a62035957f591e4f811a9ec0ab204e2a4cb414247ee48e8e4308a192911c6a"),
    (6, "e403b4182544bb35f6f682c64c234bef7b4b3ce63df762ad051c018208a3358d"),
    (11, "0c12e80f8ab1dc41609df01a6c0ba7ea6a0f99a19d8a79c30f88fcac2d1c5267"),
    (21, "f779adf8ff7b8675a3b46e78308e4148b0d6a0560feeaae7f7614893282396b8"),
    (22, "870f29a4411bfa7a57a9e27a4f64965a953b145454f2cbce92ba7f2bc1f13fe8"),
];
const WIDE_EXPECTATIONS: &[(usize, bool, &str, &str, &str)] = &[
    (
        0,
        true,
        "1835cb6b69b15dd867a050854e140de7c388e811555d3838da00d7e4487f4e0e",
        "2fa58d1b23b183dbb76cb147682f96d4588d6072cce60cd741d9f493f8f9d0d5",
        "6401b6402c3f8901cf6bd5e73dd383d4da99a6ef473727755296b0e748ecd1a9",
    ),
    (
        0,
        false,
        "a3f443ab8f5df27c0370c840ffbbe7f104ae6597c81e65b3a8b2bd273fb61f0e",
        "7379c2863a58811f5a8786eab192657d0eea971f29d018d694381b522c1c5552",
        "514f259a9073c32f8af3d41aa4caa47e3b9f1e3c64b4fa49656b7075e4310181",
    ),
    (
        1,
        true,
        "3a54db11c7ed7e5b8b0bd278f17bf4fb571cac465feebd44e7435622ab8ad39c",
        "cb4716073d19dc9832f6741954559b1e1f4b1f4a61624f217fe464ea8d8e801b",
        "80d6e6bfca092cd45cfb9f74770b9cf77f1f0c51fc7baaf5322fb837c2d71fb3",
    ),
    (
        1,
        false,
        "4f609dbf1fe8ac9665ad2dcf45697b441f63a17aa05896d480079938358f2325",
        "68e12004f202f2c6a3bcae8778427f8556d71e07cdb8a44b895c2c8d0ec276ba",
        "8c0747ba5f52cadf2b9c7e11760a9c7b2efe6d11692d55a6d0e16fd38856f063",
    ),
    (
        2,
        true,
        "7aa904b2240cdfec94e0640b355c648630a6ba4b19a8fe23b787257fb9e4c713",
        "1c2dfa00228858ba934079c20e83237eaabe37d7a6467b16f1fd308917ffd55b",
        "de15e8d4fe6b85e6b4579f9498a4f14f944ec7ceed8c6e972dab0b890548d4b5",
    ),
    (
        2,
        false,
        "f72c05ba2a51da1c56ed08865f92717193b8e9cfc2ab586960773e59ca3a0ae7",
        "a5e31aa126604a443db2472e282205873b2b25abdd740e5ab0a616c6bd000b9d",
        "c4ec971cffb066aabe505e5014659739584e14243bace2757d22a56fa9070f24",
    ),
    (
        3,
        true,
        "64e67c10b4cb5e9a091e9f32e3b658720efe56c0d8419ca3642c8855a73a8cde",
        "8d29571c3bc3b0abac175b75799c4304ae395bdeec844c706e34258ee2e942d3",
        "f82cd84e8f6997554c1351551d6637e456dbad20ad562d6d14042135c600dbba",
    ),
    (
        3,
        false,
        "90730ce53b308d4a16841cf8a3153ddb3b3a21ad7dae153d8ada7dde44d5e4e2",
        "733712e76215d0452eb178f077f637973bae17de2ead08d6b0e0ede4262ec405",
        "050d6eded2d64bf98ba386fbfed30ecbc1bc6a9cc3b7c33974914d5efddaa817",
    ),
    (
        4,
        true,
        "cd1462e682bc4dfa62f6a1b09966fd5ecce1b6ce02dde9dba572c7b447f37d05",
        "6f756914e7a6d72a400200c2d4023f6bfbc3ee2d9475f55fec6876e49bfbf481",
        "170babec4be655a96026863ff96370be580f62e3a1cdc0e10b4aff8a1653d8eb",
    ),
    (
        4,
        false,
        "af56d1de4c9fd9cfc9415a2b79f1fb61e47e10284eff9a26adbba87631f849c1",
        "4fd978242b310c5b6e866bb0d1d4c87709fe967c73b558140ef1fd6102357204",
        "5c209703fa79aa9223624e6ddf4e12e4568ab3e0588e132175e1e53a40e9b08e",
    ),
    (
        5,
        true,
        "d202e29cee9ebe5c2c9b1bd55b8e5ca15b5f547c620d53eea21681673d387259",
        "b233a50020b8c079ffc9af20c7b40a1867df65477119a6ab6c95f262621b68ce",
        "b2d78b0c92196062d4e4b8447e2f71dc87a5753e596ff4c5de39cb9ad5307b90",
    ),
    (
        5,
        false,
        "468c33a689d9c9fa6521409fec80b8df97f09900b5fec05547f1ee07bd1f1975",
        "2b4b2bc5fa56d764a43b0e919a8922ef565aa173aac7424fc3c37b1249a5e895",
        "9125d88a705fd0e726993fe8059018f25e40f9087f9562fa65f52b4d365c4574",
    ),
    (
        6,
        true,
        "bc5d2f8a83b57f820b4b3afb820e9ee218047312c6fa530fb8e069c8661522b4",
        "e855181c6d8ee3df9c369c21f89022c5914f7e9d52d5602716aa65e2cd33fe69",
        "4d94d6b75051251e167c888bbc52cc98a0c6bc07ca29925a1499a2dd4aec3042",
    ),
    (
        6,
        false,
        "147a78b8c185c6acc227be8023adf8f7bf2146eb47d0762d895305c41104f4ee",
        "63a5d7c0997754b31d10725348f1fb5d4dddead1cf776638b7d34f4633972d40",
        "0e8339396d9bd617f700a6a73d9cfead26c6e283d86d34e5b473ce606bf99223",
    ),
    (
        7,
        true,
        "415690e3e24f427ec24679709699735ee84a77c3de48f2619ea36b0bfb72a8f5",
        "23b86f0dba0c047d795b86d298489f55882f8962991c0ce7deb51a6f277fd2a5",
        "ecb339914b6aab0dc7f0952275020f44730d4b458dc9544894903002fc2dd5f8",
    ),
    (
        7,
        false,
        "10c1867a30f1d6e3290ab75b05af4ce5ee01a74e170d21130606775678856a92",
        "db58fc920f3cd135ed2302c616fab665b4a87daa6120431f4f0fa16007624a9e",
        "370b72c0ddbbca06e9815c8dbcbaa908ec9ef93b87d2e5d3211e6876d0d72743",
    ),
    (
        8,
        true,
        "85a34628518e9f3d8463ece18b76a6c9a7a8633a34ed0786649be845f5349317",
        "08c1eb97c662bee3c8dccb5b9a688808b0b5ee0ca18194941275128ba16ad1c7",
        "5a8f48a6704aa68021741f8df46c0e5fbc9a5b8b08dff2943c51764cb8d59615",
    ),
    (
        8,
        false,
        "882d49661915f645f7ca6b3accf0904cdf0d111f8fbf0665449451d0e9830113",
        "081eb2d5e3fb2a6a3b39332af02f068669dd1e35a8981949f7599c893a113c4b",
        "be038fd413c5a3750603f952835f3f35b94434f575537f873932ce61e90d1377",
    ),
    (
        9,
        true,
        "85a34628518e9f3d8463ece18b76a6c9a7a8633a34ed0786649be845f5349317",
        "08c1eb97c662bee3c8dccb5b9a688808b0b5ee0ca18194941275128ba16ad1c7",
        "5a8f48a6704aa68021741f8df46c0e5fbc9a5b8b08dff2943c51764cb8d59615",
    ),
    (
        9,
        false,
        "d5239f1b794dfc6dfa8306cdc0fe4746fbd8c6d7e28699013e22169f305665d5",
        "432097ddefb755990363f6b4d043d5a5daa097ce4e22cce3b408ba5a4586eaf7",
        "43beb619111ccf54573b97db6b957389c8f15b789ad092cdf2415fd1da50fc9e",
    ),
    (
        10,
        true,
        "cd0c4755cc9377771779ea452f7b0888ddce7a5d70c78fcf06d6829299e78a60",
        "194d487b1246baddeedba42fded29eaeabd13492a1d20d2cbbe8982958147758",
        "fd0f54051502dd9863edef3aff03b6f3b938c4ca5c694645569cc0cb4f2da59e",
    ),
    (
        10,
        false,
        "2b2a50dfa49d58949d5902673cec149608ae9ce19ba8960b786dced739305bd8",
        "261c6215b372791a29d2c1a7e7870805072c0c0ee729b88fbc059ec6a31267af",
        "fa6926e20ca70d30a768ecb1dbd78fdd5aa27d6775292599383ac2f4807e0138",
    ),
];
