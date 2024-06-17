use alloy_network::Network;
use op_alloy_rpc_types::{OpTransactionReceipt, Transaction};

impl<Provider, Pool, Network, EvmConfig> alloy_network::Network
    for EthApi<Provider, Pool, Network, EvmConfig>
{
    type TxType = todo!();
    type TxEnvelope = todo!();
    type UnsignedTx = todo!();
    type ReceiptEnvelope = todo!();
    type Header = todo!();
    type TransactionRequest = todo!();
    type TransactionResponse = Transaction;
    type ReceiptResponse = OpTransactionReceipt;
    type HeaderResponse = todo!();
}
