//! T-027: caller-owned account extension composition and required-read handoff.

use alloy_primitives::{keccak256, Address, Bytes};
use reth_db_api::{tables, transaction::DbTxMut};
use reth_primitives_traits::Account;
use reth_provider::{test_utils::create_test_provider_factory, StateProviderFactory};
use reth_storage_api::{
    AccountReader, BytecodeReader, CodeChunkReader, CodeRepresentation, DBProvider, ValidatedCode,
};

#[test]
fn t027_public_opt_in_publication_and_required_read() {
    let factory = create_test_provider_factory();
    let original = Bytes::from(vec![0; 24542]);
    let hash = keccak256(&original);
    let code = ValidatedCode::new(original.clone()).unwrap();
    let representation = CodeRepresentation::Chunked(code.descriptor().unwrap().clone());
    let account = Account {
        nonce: 43,
        bytecode_hash: Some(hash),
        extension: Bytes::from_static(&[0xa7, 0x43, 0x11]).into(),
        ..Default::default()
    };
    let writer = factory.provider_rw().unwrap();
    writer.write_chunked_code(Address::ZERO, account.clone(), &code).unwrap();
    writer.commit().unwrap();
    {
        let state = factory.latest().unwrap();
        assert_eq!(state.basic_account(&Address::ZERO).unwrap(), Some(account));
        assert_eq!(
            state.get_required_code_chunk(&hash, &representation, 1).unwrap(),
            Some(Bytes::from_static(&[0]))
        );
        assert_eq!(state.bytecode_by_hash(&hash).unwrap().unwrap().original_bytes(), original);
    }
    let writer = factory.provider_rw().unwrap();
    assert!(writer.tx_ref().delete::<tables::BytecodeChunks>(keccak256([0]), None).unwrap());
    writer.commit().unwrap();
    assert!(factory.latest().unwrap().get_required_code_chunk(&hash, &representation, 1).is_err());
}
