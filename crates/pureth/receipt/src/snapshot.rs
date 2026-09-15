use crate::{
    tree::{
        merkleize_fixed, merkleize_progressive, mix_in_length, progressive_byte_list, RetainedNode,
        TreeConstructionError,
    },
    LogSsz, ReceiptSsz, ReceiptsSsz, MAX_TOPICS,
};
use alloy_primitives::{Address, B256};
use tree_hash::TreeHash;

#[derive(Debug)]
pub struct ReceiptSnapshot {
    receipts: ReceiptsSsz,
    tree: RetainedNode,
}

impl ReceiptSnapshot {
    pub fn build(receipts: ReceiptsSsz) -> Result<Self, TreeConstructionError> {
        let tree = build_receipts_tree(&receipts)?;
        Ok(Self { receipts, tree })
    }

    pub const fn receipts(&self) -> &ReceiptsSsz {
        &self.receipts
    }

    pub const fn tree(&self) -> &RetainedNode {
        &self.tree
    }

    pub const fn root(&self) -> B256 {
        self.tree.root()
    }
}

fn build_receipts_tree(receipts: &ReceiptsSsz) -> Result<RetainedNode, TreeConstructionError> {
    let nodes = receipts.0.iter().map(build_receipt_tree).collect::<Result<Vec<_>, _>>()?;

    Ok(mix_in_length(merkleize_progressive(nodes)?, receipts.len()))
}

fn build_receipt_tree(receipt: &ReceiptSsz) -> Result<RetainedNode, TreeConstructionError> {
    let log_nodes = receipt.logs.iter().map(build_log_tree).collect::<Result<Vec<_>, _>>()?;

    let logs = mix_in_length(merkleize_progressive(log_nodes)?, receipt.logs.len());

    merkleize_fixed(
        vec![
            RetainedNode::leaf(receipt.tx_type.tree_hash_root()),
            RetainedNode::leaf(receipt.success.tree_hash_root()),
            RetainedNode::leaf(receipt.gas_used.tree_hash_root()),
            optional_address_tree(receipt.contract_address),
            logs,
        ],
        8,
    )
}

fn build_log_tree(log: &LogSsz) -> Result<RetainedNode, TreeConstructionError> {
    merkleize_fixed(
        vec![
            RetainedNode::leaf(log.address.tree_hash_root()),
            topics_tree(&log.topics)?,
            progressive_byte_list(log.data.as_ref())?,
        ],
        4,
    )
}

fn topics_tree(topics: &[B256]) -> Result<RetainedNode, TreeConstructionError> {
    let nodes = topics.iter().map(|topic| RetainedNode::leaf(topic.tree_hash_root())).collect();

    Ok(mix_in_length(merkleize_fixed(nodes, MAX_TOPICS)?, topics.len()))
}

fn optional_address_tree(address: Option<Address>) -> RetainedNode {
    match address {
        None => RetainedNode::pair(RetainedNode::zero(), RetainedNode::leaf(0_u8.tree_hash_root())),
        Some(address) => RetainedNode::pair(
            RetainedNode::leaf(address.tree_hash_root()),
            RetainedNode::leaf(1_u8.tree_hash_root()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_fields_use_exact_little_endian_chunks() {
        for (tx_type, success, gas_used) in
            [(0_u8, false, 0_u64), (5, true, 0x0102_0304_0506_0708), (u8::MAX, true, u64::MAX)]
        {
            let receipt =
                ReceiptSsz { tx_type, success, gas_used, contract_address: None, logs: Vec::new() };
            let tree = build_receipt_tree(&receipt).unwrap();
            let fields = tree.children().unwrap()[0].children().unwrap();
            let first = fields[0].children().unwrap();
            let second = fields[1].children().unwrap();
            let mut chunk = [0_u8; 32];
            chunk[0] = tx_type;
            assert_eq!(first[0].root(), B256::from(chunk));
            chunk[0] = u8::from(success);
            assert_eq!(first[1].root(), B256::from(chunk));
            chunk[..8].copy_from_slice(&gas_used.to_le_bytes());
            assert_eq!(second[0].root(), B256::from(chunk));
        }
    }

    #[test]
    fn optional_address_retains_value_and_selector() {
        for address in [None, Some(Address::ZERO), Some(Address::repeat_byte(0x11))] {
            let tree = optional_address_tree(address);
            let children = tree.children().unwrap();
            let mut value = [0_u8; 32];
            let mut selector = [0_u8; 32];
            if let Some(address) = address {
                value[..20].copy_from_slice(address.as_slice());
                selector[0] = 1;
            }
            assert_eq!(children[0].root(), B256::from(value));
            assert_eq!(children[1].root(), B256::from(selector));
            let mut encoded = [0_u8; 64];
            encoded[..32].copy_from_slice(&value);
            encoded[32..].copy_from_slice(&selector);
            assert_eq!(tree.root(), tree_hash::merkle_root(&encoded, 2));
        }
        assert_ne!(
            optional_address_tree(None).root(),
            optional_address_tree(Some(Address::ZERO)).root()
        );
    }

    #[test]
    fn topics_retain_fixed_contents_and_logical_length() {
        for count in 0..=MAX_TOPICS {
            let topics = (0..count)
                .map(|i| B256::repeat_byte(u8::try_from(i + 1).unwrap()))
                .collect::<Vec<_>>();
            let tree = topics_tree(&topics).unwrap();
            let children = tree.children().unwrap();
            let encoded = topics.iter().flat_map(|topic| topic.iter().copied()).collect::<Vec<_>>();
            let contents = tree_hash::merkle_root(&encoded, MAX_TOPICS);
            assert_eq!(children[0].root(), contents);
            let mut length = [0_u8; 32];
            length[0] = u8::try_from(count).unwrap();
            assert_eq!(children[1].root(), B256::from(length));
            assert_eq!(tree.root(), tree_hash::mix_in_length(&contents, count));
            let pairs = children[0].children().unwrap();
            for (i, expected) in topics.iter().enumerate() {
                assert_eq!(pairs[i / 2].children().unwrap()[i % 2].root(), *expected);
            }
        }
        let a = B256::repeat_byte(0x11);
        let b = B256::repeat_byte(0x22);
        assert_ne!(topics_tree(&[a]).unwrap().root(), topics_tree(&[b]).unwrap().root());
        assert_ne!(topics_tree(&[a, b]).unwrap().root(), topics_tree(&[b, a]).unwrap().root());
        assert_ne!(topics_tree(&[]).unwrap().root(), topics_tree(&[B256::ZERO]).unwrap().root());
    }
}
