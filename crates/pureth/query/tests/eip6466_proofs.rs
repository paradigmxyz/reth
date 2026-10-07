#![allow(missing_docs)]

use alloy_primitives::{Address, Bytes, B256};
use reth_pureth_query::{
    address_target_node, branch_positions, compose_gindices, container_field_gindex,
    progressive_chunk_gindex, select_receipt_snapshot, verify_branch, ReceiptSelection,
    SelectionError, SelectionLimits, SelectionOperation, SelectionRequest,
};
use reth_pureth_receipt::{
    eip6466::{
        BasicReceipt, CreateReceipt, Eip6466ReceiptSnapshot, Log, Receipt, Receipts, SetCodeReceipt,
    },
    RetainedNode,
};

#[derive(Clone, Copy)]
enum ResolvedPath {
    ReceiptLogAddress { receipt_index: u64, log_index: u64 },
}

fn receipts(count: usize, log_count: usize) -> Vec<Receipt> {
    (0..count)
        .map(|index| {
            let logs = (0..log_count)
                .map(|log_index| {
                    let mut address = [0_u8; 20];
                    address[0] = u8::try_from(index).unwrap();
                    address[1] = u8::try_from(log_index).unwrap();
                    Log::new(Address::from(address), vec![], Bytes::from_static(&[1, 2, 3]))
                        .unwrap()
                })
                .collect();
            match index % 3 {
                0 => Receipt::Basic(BasicReceipt {
                    from_: Address::repeat_byte(0x11),
                    gas_used: 21_000,
                    logs,
                    status: true,
                }),
                1 => Receipt::Create(CreateReceipt {
                    from_: Address::repeat_byte(0x11),
                    gas_used: 21_000,
                    contract_address: Address::repeat_byte(0x22),
                    logs,
                    status: true,
                }),
                _ => Receipt::SetCode(SetCodeReceipt {
                    from_: Address::repeat_byte(0x11),
                    gas_used: 21_000,
                    logs,
                    status: true,
                    authorities: vec![Address::repeat_byte(0x33), Address::ZERO],
                }),
            }
        })
        .collect()
}

fn snapshot(receipts: Vec<Receipt>) -> Eip6466ReceiptSnapshot {
    Eip6466ReceiptSnapshot::build(Receipts::new(receipts)).unwrap()
}

fn address_gindex(path: ResolvedPath) -> u128 {
    let ResolvedPath::ReceiptLogAddress { receipt_index, log_index } = path;
    [
        progressive_chunk_gindex(receipt_index).unwrap(),
        2,
        progressive_chunk_gindex(3).unwrap(),
        progressive_chunk_gindex(log_index).unwrap(),
        container_field_gindex(3, 0).unwrap(),
    ]
    .into_iter()
    .try_fold(1, compose_gindices)
    .unwrap()
}

fn branch(tree: &RetainedNode, gindex: u128) -> (B256, Vec<B256>) {
    let mut node = tree;
    let mut siblings = Vec::new();
    for bit in (0..u128::BITS - 1 - gindex.leading_zeros()).rev() {
        let children = node.children().expect("address path must be retained");
        let side = usize::from(gindex & (1 << bit) != 0);
        siblings.push(children[side ^ 1].root());
        node = &children[side];
    }
    siblings.reverse();
    (node.root(), siblings)
}

fn selected_address(snapshot: &Eip6466ReceiptSnapshot, path: ResolvedPath) -> Option<Address> {
    let ResolvedPath::ReceiptLogAddress { receipt_index, log_index } = path;
    let receipt = snapshot.receipts().get(usize::try_from(receipt_index).ok()?)?;
    receipt.logs().get(usize::try_from(log_index).ok()?).map(Log::address)
}

fn proof(snapshot: &Eip6466ReceiptSnapshot, path: ResolvedPath) -> (B256, u128, Vec<B256>) {
    let address = selected_address(snapshot, path).unwrap();
    let target = address_target_node(address.as_slice()).unwrap();
    let gindex = address_gindex(path);
    let (node, branch) = branch(snapshot.tree(), gindex);
    assert_eq!(node, target);
    verify_branch(target, gindex, &branch, snapshot.root()).unwrap();
    (target, gindex, branch)
}

fn changed(mut node: B256) -> B256 {
    node[0] ^= 1;
    node
}

#[test]
fn eip_address_proofs_use_the_retained_tree_for_each_receipt_variant() {
    let snapshot = snapshot(receipts(3, 2));
    assert_eq!(snapshot.root(), snapshot.tree().root());
    for receipt_index in 0..3 {
        for log_index in 0..2 {
            proof(&snapshot, ResolvedPath::ReceiptLogAddress { receipt_index, log_index });
        }
    }
}

#[test]
fn eip_address_proofs_cover_progressive_receipt_and_log_boundaries() {
    let snapshot = snapshot(receipts(22, 22));
    for receipt_index in [0, 1, 4, 5, 20, 21] {
        for log_index in [0, 1, 4, 5, 20, 21] {
            proof(&snapshot, ResolvedPath::ReceiptLogAddress { receipt_index, log_index });
        }
    }
}

#[test]
fn eip_branches_include_lengths_selectors_and_active_fields() {
    let snapshot = snapshot(receipts(3, 2));
    for (receipt_index, selector_byte, active_byte) in [(0, 1, 0x1b), (1, 2, 0x1f), (2, 3, 0x3b)] {
        let (_, gindex, proof) =
            proof(&snapshot, ResolvedPath::ReceiptLogAddress { receipt_index, log_index: 1 });
        let receipt_gindex = progressive_chunk_gindex(receipt_index).unwrap();
        let container_gindex = compose_gindices(receipt_gindex, 2).unwrap();
        let logs_gindex =
            compose_gindices(container_gindex, progressive_chunk_gindex(3).unwrap()).unwrap();
        let positions = branch_positions(gindex).unwrap();
        let mut receipt_length = B256::ZERO;
        receipt_length[..8].copy_from_slice(&3_u64.to_le_bytes());
        let mut log_length = B256::ZERO;
        log_length[..8].copy_from_slice(&2_u64.to_le_bytes());
        let mut selector = B256::ZERO;
        selector[0] = selector_byte;
        let mut active_fields = B256::ZERO;
        active_fields[0] = active_byte;
        for (position, expected) in [
            (3, receipt_length),
            (compose_gindices(logs_gindex, 3).unwrap(), log_length),
            (compose_gindices(receipt_gindex, 3).unwrap(), selector),
            (compose_gindices(container_gindex, 3).unwrap(), active_fields),
        ] {
            let index = positions.iter().position(|actual| *actual == position).unwrap();
            assert_eq!(proof[index], expected);
        }
    }
}

#[test]
fn eip_proofs_reject_changed_values_roots_branches_and_positions() {
    let snapshot = snapshot(receipts(3, 2));
    for receipt_index in 0..3 {
        let (target, gindex, branch) =
            proof(&snapshot, ResolvedPath::ReceiptLogAddress { receipt_index, log_index: 1 });
        let root = snapshot.root();
        assert!(verify_branch(changed(target), gindex, &branch, root).is_err());
        assert!(verify_branch(target, gindex, &branch, changed(root)).is_err());
        assert!(verify_branch(target, gindex ^ 1, &branch, root).is_err());
        assert!(verify_branch(target, gindex, &branch[1..], root).is_err());
        let mut reversed = branch.clone();
        reversed.reverse();
        assert!(verify_branch(target, gindex, &reversed, root).is_err());
        for index in 0..branch.len() {
            let mut altered = branch.clone();
            altered[index] = changed(altered[index]);
            assert!(verify_branch(target, gindex, &altered, root).is_err());
        }
    }
}

#[test]
fn receipt_selection_rejects_out_of_bounds_indexes() {
    let reject = |snapshot: &Eip6466ReceiptSnapshot, receipt_index: u64, log_index: u64| {
        let request = SelectionRequest {
            selections: vec![ReceiptSelection {
                path: format!("[{receipt_index}].logs[{log_index}].address"),
                operation: SelectionOperation::Value {},
            }],
            include_proof: true,
        };
        assert_eq!(
            select_receipt_snapshot(snapshot, &request, SelectionLimits::default()),
            Err(SelectionError::OutOfBounds)
        );
    };
    for snapshot in [snapshot(receipts(0, 0)), snapshot(receipts(3, 0)), snapshot(receipts(3, 2))] {
        reject(&snapshot, snapshot.receipts().len() as u64, 0);
        for receipt_index in 0..snapshot.receipts().len() as u64 {
            for log_index in [2, u64::MAX] {
                reject(&snapshot, receipt_index, log_index);
            }
        }
    }
    reject(&snapshot(receipts(3, 0)), 0, 0);
}

#[test]
fn eip_old_proofs_reject_changed_status_data_authorities_and_order() {
    let original = receipts(3, 2);
    let snapshot = snapshot(original.clone());
    let (target, gindex, branch) =
        proof(&snapshot, ResolvedPath::ReceiptLogAddress { receipt_index: 0, log_index: 0 });
    for change in 0..5 {
        let mut altered = original.clone();
        match change {
            0 => {
                let Receipt::Basic(receipt) = &mut altered[0] else { unreachable!() };
                receipt.status = false;
            }
            1 => {
                let Receipt::Basic(receipt) = &mut altered[0] else { unreachable!() };
                receipt.logs[0] =
                    Log::new(receipt.logs[0].address(), vec![], Bytes::from_static(&[4])).unwrap();
            }
            2 => {
                let Receipt::SetCode(receipt) = &mut altered[2] else { unreachable!() };
                receipt.authorities[0] = Address::repeat_byte(0x44);
            }
            3 => altered.reverse(),
            _ => {
                let Receipt::Basic(receipt) = &mut altered[0] else { unreachable!() };
                receipt.logs.reverse();
            }
        }
        let root = Eip6466ReceiptSnapshot::build(Receipts::new(altered)).unwrap().root();
        assert_ne!(root, snapshot.root());
        assert!(verify_branch(target, gindex, &branch, root).is_err());
    }
}
