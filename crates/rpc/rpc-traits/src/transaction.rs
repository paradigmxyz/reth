use alloy_consensus::{error::ValueError, transaction::Recovered, EthereumTxEnvelope, TxEip4844};
use alloy_primitives::Address;
use alloy_rpc_types_eth::{Transaction, TransactionInfo, TransactionRequest};
use core::{convert::Infallible, error};

/// Converts `T` into `self`.
///
/// Should create an RPC transaction response object based on a consensus transaction, its signer
/// [`Address`] and an additional context [`FromConsensusTx::TxInfo`].
pub trait FromConsensusTx<T>: Sized {
    /// An additional context, usually [`TransactionInfo`] in a wrapper that carries some
    /// implementation specific extra information.
    type TxInfo;
    /// An associated RPC conversion error.
    type Err: error::Error;

    /// Performs the conversion consuming `tx` with `signer` and `tx_info`. See [`FromConsensusTx`]
    /// for details.
    fn from_consensus_tx(tx: T, signer: Address, tx_info: Self::TxInfo) -> Result<Self, Self::Err>;
}

impl<TxIn: alloy_consensus::Transaction, T: alloy_consensus::Transaction + From<TxIn>>
    FromConsensusTx<TxIn> for Transaction<T>
{
    type TxInfo = TransactionInfo;
    type Err = Infallible;

    fn from_consensus_tx(
        tx: TxIn,
        signer: Address,
        tx_info: Self::TxInfo,
    ) -> Result<Self, Self::Err> {
        Ok(Self::from_transaction(Recovered::new_unchecked(tx.into(), signer), tx_info))
    }
}

/// Converts `self` into `T`.
///
/// Should create a fake transaction for simulation using [`TransactionRequest`].
pub trait TryIntoSimTx<T>
where
    Self: Sized,
{
    /// Performs the conversion.
    ///
    /// Should return a signed typed transaction envelope for the [`eth_simulateV1`] endpoint with a
    /// dummy signature or an error if [required fields] are missing.
    ///
    /// [`eth_simulateV1`]: <https://github.com/ethereum/execution-apis/pull/484>
    /// [required fields]: TransactionRequest::buildable_type
    fn try_into_sim_tx(self) -> Result<T, ValueError<Self>>;
}

impl TryIntoSimTx<EthereumTxEnvelope<TxEip4844>> for TransactionRequest {
    fn try_into_sim_tx(self) -> Result<EthereumTxEnvelope<TxEip4844>, ValueError<Self>> {
        Self::build_typed_simulate_transaction(self)
    }
}

/// Adds extra context to [`TransactionInfo`].
pub trait TxInfoMapper<T> {
    /// An associated output type that carries [`TransactionInfo`] with some extra context.
    type Out;
    /// An associated error that can occur during the mapping.
    type Err;

    /// Performs the conversion.
    fn try_map(&self, tx: &T, tx_info: TransactionInfo) -> Result<Self::Out, Self::Err>;
}

impl<T> TxInfoMapper<T> for () {
    type Out = TransactionInfo;
    type Err = Infallible;

    fn try_map(&self, _tx: &T, tx_info: TransactionInfo) -> Result<Self::Out, Self::Err> {
        Ok(tx_info)
    }
}
