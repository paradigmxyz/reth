use reth_interfaces::{consensus::Error, Result as RethResult};
use reth_primitives::{ChainSpec, SealedBlock};
use reth_provider::{AccountProvider, HeaderProvider};

mod block;
mod header;
mod transaction;

pub use block::{
    calculate_next_block_base_fee, validate_block_regarding_chain, validate_block_standalone,
};
pub use header::{
    clique, validate_eip_1559_base_fee, validate_gas_limit_difference, validate_header_consistency,
    validate_header_regarding_parent, validate_header_standalone,
};
pub use transaction::{
    validate_all_transaction_regarding_block_and_nonces, validate_transaction_regarding_header,
};

/// Full validation of block before execution.
pub fn full_validation<Provider: HeaderProvider + AccountProvider>(
    block: &SealedBlock,
    provider: Provider,
    chain_spec: &ChainSpec,
) -> RethResult<()> {
    validate_header_standalone(&block.header, chain_spec)?;
    validate_block_standalone(block)?;
    let parent = validate_block_regarding_chain(block, &provider)?;
    validate_header_regarding_parent(&parent, &block.header, chain_spec)?;

    // NOTE: depending on the need of the stages, recovery could be done in different place.
    let transactions = block
        .body
        .iter()
        .map(|tx| tx.try_ecrecovered().ok_or(Error::TransactionSignerRecoveryError))
        .collect::<Result<Vec<_>, _>>()?;

    validate_all_transaction_regarding_block_and_nonces(
        transactions.iter(),
        &block.header,
        provider,
        chain_spec,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use reth_interfaces::{consensus::Error, Result};
    use reth_primitives::{
        hex_literal::hex, Account, Address, BlockHash, Bytes, Header, SealedBlock, Signature,
        Transaction, TransactionKind, TransactionSigned, TransactionSignedEcRecovered, TxEip2930,
        MAINNET, U256,
    };
    use reth_provider::{AccountProvider, HeaderProvider};

    use super::*;

    struct Provider {
        is_known: bool,
        parent: Option<Header>,
        account: Option<Account>,
    }

    impl Provider {
        /// New provider with parent
        fn new(parent: Option<Header>) -> Self {
            Self { is_known: false, parent, account: None }
        }
        /// New provider where is_known is always true
        fn new_known() -> Self {
            Self { is_known: true, parent: None, account: None }
        }
    }

    impl AccountProvider for Provider {
        fn basic_account(&self, _address: Address) -> Result<Option<Account>> {
            Ok(self.account)
        }
    }

    impl HeaderProvider for Provider {
        fn is_known(&self, _block_hash: &BlockHash) -> Result<bool> {
            Ok(self.is_known)
        }

        fn header(&self, _block_number: &BlockHash) -> Result<Option<Header>> {
            Ok(self.parent.clone())
        }

        fn header_by_number(&self, _num: u64) -> Result<Option<Header>> {
            Ok(self.parent.clone())
        }

        fn header_td(&self, _hash: &BlockHash) -> Result<Option<U256>> {
            Ok(None)
        }
    }

    fn mock_tx(nonce: u64) -> TransactionSignedEcRecovered {
        let request = Transaction::Eip2930(TxEip2930 {
            chain_id: 1u64,
            nonce,
            gas_price: 0x28f000fff,
            gas_limit: 10,
            to: TransactionKind::Call(Address::default()),
            value: 3,
            input: Bytes::from(vec![1, 2]),
            access_list: Default::default(),
        });

        let signature = Signature { odd_y_parity: true, r: U256::default(), s: U256::default() };

        let tx = TransactionSigned::from_transaction_and_signature(request, signature);
        let signer = Address::zero();
        TransactionSignedEcRecovered::from_signed_transaction(tx, signer)
    }
    /// got test block
    fn mock_block() -> (SealedBlock, Header) {
        // https://etherscan.io/block/15867168 where transaction root and receipts root are cleared
        // empty merkle tree: 0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421

        let header = Header {
            parent_hash: hex!("859fad46e75d9be177c2584843501f2270c7e5231711e90848290d12d7c6dcdd").into(),
            ommers_hash: hex!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347").into(),
            beneficiary: hex!("4675c7e5baafbffbca748158becba61ef3b0a263").into(),
            state_root: hex!("8337403406e368b3e40411138f4868f79f6d835825d55fd0c2f6e17b1a3948e9").into(),
            transactions_root: hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").into(),
            receipts_root: hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").into(),
            logs_bloom: hex!("002400000000004000220000800002000000000000000000000000000000100000000000000000100000000000000021020000000800000006000000002100040000000c0004000000000008000008200000000000000000000000008000000001040000020000020000002000000800000002000020000000022010000000000000010002001000000000020200000000000001000200880000004000000900020000000000020000000040000000000000000000000000000080000000000001000002000000000000012000200020000000000000001000000000000020000010321400000000100000000000000000000000000000400000000000000000").into(),
            difficulty: U256::ZERO, // total difficulty: 0xc70d815d562d3cfa955).into(),
            number: 0xf21d20,
            gas_limit: 0x1c9c380,
            gas_used: 0x6e813,
            timestamp: 0x635f9657,
            extra_data: hex!("")[..].into(),
            mix_hash: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
            nonce: 0x0000000000000000,
            base_fee_per_gas: 0x28f0001df.into(),
        };
        // size: 0x9b5

        let mut parent = header.clone();
        parent.gas_used = 17763076;
        parent.gas_limit = 30000000;
        parent.base_fee_per_gas = Some(0x28041f7f5);
        parent.number -= 1;

        let ommers = Vec::new();
        let body = Vec::new();

        (SealedBlock { header: header.seal(), body, ommers }, parent)
    }

    #[test]
    fn sanity_check() {
        let (block, parent) = mock_block();
        let provider = Provider::new(Some(parent));

        assert_eq!(full_validation(&block, provider, &MAINNET), Ok(()), "Validation should pass");
    }

    #[test]
    fn validate_known_block() {
        let (block, _) = mock_block();
        let provider = Provider::new_known();

        assert_eq!(
            full_validation(&block, provider, &MAINNET),
            Err(Error::BlockKnown { hash: block.hash(), number: block.number }.into()),
            "Should fail with error"
        );
    }

    #[test]
    fn sanity_tx_nonce_check() {
        let (block, _) = mock_block();
        let tx1 = mock_tx(0);
        let tx2 = mock_tx(1);
        let provider = Provider::new_known();

        let txs = vec![tx1, tx2];
        validate_all_transaction_regarding_block_and_nonces(
            txs.iter(),
            &block.header,
            provider,
            &MAINNET,
        )
        .expect("To Pass");
    }

    #[test]
    fn nonce_gap_in_first_transaction() {
        let (block, _) = mock_block();
        let tx1 = mock_tx(1);
        let provider = Provider::new_known();

        let txs = vec![tx1];
        assert_eq!(
            validate_all_transaction_regarding_block_and_nonces(
                txs.iter(),
                &block.header,
                provider,
                &MAINNET,
            ),
            Err(Error::TransactionNonceNotConsistent.into())
        )
    }

    #[test]
    fn nonce_gap_on_second_tx_from_same_signer() {
        let (block, _) = mock_block();
        let tx1 = mock_tx(0);
        let tx2 = mock_tx(3);
        let provider = Provider::new_known();

        let txs = vec![tx1, tx2];
        assert_eq!(
            validate_all_transaction_regarding_block_and_nonces(
                txs.iter(),
                &block.header,
                provider,
                &MAINNET,
            ),
            Err(Error::TransactionNonceNotConsistent.into())
        );
    }
}
