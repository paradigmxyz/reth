use crate::{
    codec::{authorization_selector, authorization_signature_bytes, signature_bytes},
    decode_transactions, encode_transactions, PayloadView, TransactionAuthorization,
    TransactionError, TransactionSsz,
};
use alloy_eips::{eip2930::AccessList, eip7702::constants::MAGIC};
use alloy_primitives::{Address, Bytes, B256, U256};
use reth_pureth_ssz::{merkleize_progressive, mix_in_length, progressive_byte_list, RetainedNode};

#[derive(Debug)]
pub struct TransactionSnapshot {
    transactions: Vec<TransactionSsz>,
    serialized: Bytes,
    tree: RetainedNode,
}

impl TransactionSnapshot {
    pub fn build(transactions: Vec<TransactionSsz>) -> Result<Self, TransactionError> {
        let serialized = Bytes::from(encode_transactions(&transactions)?);
        let nodes = transactions.iter().map(transaction_tree).collect::<Result<Vec<_>, _>>()?;
        let contents = merkleize_progressive(nodes).map_err(TransactionError::Tree)?;
        let tree = mix_in_length(contents, transactions.len());
        Ok(Self { transactions, serialized, tree })
    }

    pub fn from_ssz(bytes: &[u8]) -> Result<Self, TransactionError> {
        Self::build(decode_transactions(bytes)?)
    }

    pub fn transactions(&self) -> &[TransactionSsz] {
        &self.transactions
    }

    pub fn serialized(&self) -> &[u8] {
        self.serialized.as_ref()
    }

    pub const fn tree(&self) -> &RetainedNode {
        &self.tree
    }

    pub const fn root(&self) -> B256 {
        self.tree.root()
    }

    pub fn transaction_tree(&self, index: usize) -> Option<&RetainedNode> {
        self.transactions.get(index)?;
        let contents = &self.tree.children()?[0];
        progressive_element(contents, index, 1)
    }
}

fn transaction_tree(transaction: &TransactionSsz) -> Result<RetainedNode, TransactionError> {
    let p = PayloadView::from(transaction.payload());
    let mut fields = vec![
        word_node(u64::from(transaction.payload().transaction_type())),
        p.chain_id.map_or_else(RetainedNode::zero, uint256_node),
        word_node(p.nonce),
        fees_tree(p.fee, p.blob_fee)?,
        word_node(p.gas),
        p.to.map_or_else(RetainedNode::zero, address_node),
        uint256_node(p.value),
        progressive_byte_list(p.input).map_err(TransactionError::Tree)?,
    ];
    if let Some(access) = p.access_list {
        fields.push(access_list_tree(access)?);
    }
    if let Some(priority) = p.priority_fee {
        fields.push(fees_tree(priority, None)?);
    }
    if let Some(hashes) = p.blob_hashes {
        fields.push(hashes_tree(hashes)?);
    }
    if let Some(authorizations) = p.authorizations {
        fields.push(RetainedNode::zero());
        fields.push(list_tree(
            authorizations.iter().map(authorization_tree).collect::<Result<Vec<_>, _>>()?,
        )?);
    }
    let active_fields = match transaction.selector() {
        0x01 => 0xfd,
        0x02 => 0xdd,
        0x03 => 0xff,
        0x04 => 0xdf,
        0x05 => 0x1ff,
        0x06 => 0x1df,
        0x07 => 0x3ff,
        0x08 => 0x3df,
        0x09 => 0x7ff,
        0x0a => 0xbff,
        selector => return Err(TransactionError::UnsupportedSelector(selector)),
    };
    let payload = RetainedNode::pair(
        merkleize_progressive(fields).map_err(TransactionError::Tree)?,
        word_node(active_fields),
    );
    let payload = RetainedNode::pair(payload, word_node(u64::from(transaction.selector())));
    let signature = progressive_byte_list(&signature_bytes(transaction.signature()))
        .map_err(TransactionError::Tree)?;
    Ok(RetainedNode::pair(payload, signature))
}

fn fees_tree(regular: U256, blob: Option<U256>) -> Result<RetainedNode, TransactionError> {
    let mut fields = vec![uint256_node(regular)];
    if let Some(blob) = blob {
        fields.push(uint256_node(blob));
    }
    let mask = if blob.is_some() { 3 } else { 1 };
    Ok(RetainedNode::pair(
        merkleize_progressive(fields).map_err(TransactionError::Tree)?,
        word_node(mask),
    ))
}

fn list_tree(nodes: Vec<RetainedNode>) -> Result<RetainedNode, TransactionError> {
    let count = nodes.len();
    Ok(mix_in_length(merkleize_progressive(nodes).map_err(TransactionError::Tree)?, count))
}

fn hashes_tree(hashes: &[B256]) -> Result<RetainedNode, TransactionError> {
    list_tree(hashes.iter().copied().map(RetainedNode::leaf).collect())
}

fn access_list_tree(access: &AccessList) -> Result<RetainedNode, TransactionError> {
    list_tree(
        access
            .0
            .iter()
            .map(|item| {
                Ok(RetainedNode::pair(address_node(item.address), hashes_tree(&item.storage_keys)?))
            })
            .collect::<Result<Vec<_>, TransactionError>>()?,
    )
}

fn authorization_tree(auth: &TransactionAuthorization) -> Result<RetainedNode, TransactionError> {
    let inner = auth.inner();
    let fields = vec![
        word_node(u64::from(MAGIC)),
        if inner.chain_id.is_zero() { RetainedNode::zero() } else { uint256_node(inner.chain_id) },
        address_node(inner.address),
        word_node(inner.nonce),
    ];
    let mask = if inner.chain_id.is_zero() { 0x0d } else { 0x0f };
    let payload = RetainedNode::pair(
        merkleize_progressive(fields).map_err(TransactionError::Tree)?,
        word_node(mask),
    );
    let payload = RetainedNode::pair(payload, word_node(u64::from(authorization_selector(auth))));
    let signature = progressive_byte_list(&authorization_signature_bytes(auth))
        .map_err(TransactionError::Tree)?;
    Ok(RetainedNode::pair(payload, signature))
}

fn address_node(address: Address) -> RetainedNode {
    RetainedNode::leaf(B256::right_padding_from(address.as_slice()))
}

fn uint256_node(value: U256) -> RetainedNode {
    RetainedNode::leaf(B256::from(value.to_le_bytes::<32>()))
}

fn word_node(value: u64) -> RetainedNode {
    RetainedNode::leaf(B256::right_padding_from(&value.to_le_bytes()))
}

fn progressive_element(node: &RetainedNode, index: usize, width: usize) -> Option<&RetainedNode> {
    let children = node.children()?;
    if index < width {
        return fixed_element(&children[0], index, width);
    }
    progressive_element(&children[1], index.checked_sub(width)?, width.checked_mul(4)?)
}

fn fixed_element(node: &RetainedNode, index: usize, width: usize) -> Option<&RetainedNode> {
    if width == 1 {
        return Some(node);
    }
    let children = node.children()?;
    let half = width / 2;
    if index < half {
        fixed_element(&children[0], index, half)
    } else {
        fixed_element(&children[1], index - half, half)
    }
}
