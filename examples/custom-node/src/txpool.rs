use reth_optimism_node::txpool::OpTransactionPool;
use reth_transaction_pool::blobstore::DiskFileBlobStore;

pub type CustomTxPool<Provider> = OpTransactionPool<Provider, DiskFileBlobStore>;
