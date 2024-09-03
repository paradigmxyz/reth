#![allow(missing_docs)]

use reth_db_api::Database;
use reth_primitives::BlockHashOrNumber;
use reth_provider::{
    BlockNumReader, BlockReader, HeaderProvider, ProviderResult, RequestsProvider,
    TransactionsProvider, WithdrawalsProvider,
};
use std::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

/// Mock Primitives trait
pub trait Primitives {
    type Block: reth_primitives_traits::Block;
}

/// Mock NodeTypes trait
pub trait NodeTypes {
    type Primitives: Primitives;
    type Chainspec;
    type Storage: NodeStorage<NodeTypes = Self>;
}

/// Making this stateful would move the DB bounds to whatever type implements this. eg
/// EthStorage<Provider<DB,NT>>
pub trait NodeStorage {
    type NodeTypes: NodeTypes<Storage = Self>;

    fn block<DB: Database>(
        provider: &DatabaseProvider<DB, Self::NodeTypes>,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<<<Self::NodeTypes as NodeTypes>::Primitives as Primitives>::Block>>;
}

/*
    Mocks DatabaseProvider, Block Reader trait and their generic impl.
*/

#[auto_impl::auto_impl(&, Arc)]
pub trait BlockReader2<Block: reth_primitives_traits::Block>: Send + Sync {
    /// Returns the block with given id from the database.
    ///
    /// Returns `None` if block is not found.
    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Block>>;
}

/// Mock DatabaseProvider<DB, NodeTypes>
#[derive(Debug)]
pub struct DatabaseProvider<DB: Database, NT>(
    pub (reth_provider::DatabaseProvider<<DB as Database>::TXMut>, PhantomData<NT>),
);

impl<DB: Database, NT> Deref for DatabaseProvider<DB, NT> {
    type Target = reth_provider::DatabaseProvider<<DB as Database>::TXMut>;

    fn deref(&self) -> &Self::Target {
        &self.0 .0
    }
}

impl<DB: Database, NT> DerefMut for DatabaseProvider<DB, NT> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0 .0
    }
}

impl<DB: Database, NT: NodeTypes + Send + Sync> BlockReader2<<NT::Primitives as Primitives>::Block>
    for DatabaseProvider<DB, NT>
{
    fn block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<<NT::Primitives as Primitives>::Block>> {
        <<NT as NodeTypes>::Storage as NodeStorage>::block(self, id)
    }
}

/*
    Chain specific implementation
*/

/// Implements Eth
#[derive(Debug)]
pub struct EthPrimitives;

/// EthStorage
#[derive(Debug)]
pub struct EthStorage;

/// EthereumNode
#[derive(Debug)]
pub struct EthereumNode;

impl Primitives for EthPrimitives {
    type Block = reth_primitives::Block;
}

impl NodeTypes for EthereumNode {
    type Primitives = EthPrimitives;
    type Chainspec = ();
    type Storage = EthStorage;
}

impl NodeStorage for EthStorage {
    type NodeTypes = EthereumNode;

    fn block<DB: Database>(
        provider: &DatabaseProvider<DB, Self::NodeTypes>,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<reth_primitives::Block>> {
        if let Some(number) = provider.convert_hash_or_number(id)? {
            if let Some(header) = provider.header_by_number(number)? {
                let withdrawals = provider.withdrawals_by_block(number.into(), header.timestamp)?;
                let ommers = provider.ommers(number.into())?.unwrap_or_default();
                let requests = provider.requests_by_block(number.into(), header.timestamp)?;
                // If the body indices are not found, this means that the transactions either do not
                // exist in the database yet, or they do exit but are not indexed.
                // If they exist but are not indexed, we don't have enough
                // information to return the block anyways, so we return `None`.
                let transactions = match provider.transactions_by_block(number.into())? {
                    Some(transactions) => transactions,
                    None => return Ok(None),
                };

                return Ok(Some(reth_primitives::Block {
                    header,
                    body: transactions,
                    ommers,
                    withdrawals,
                    requests,
                }))
            }
        }

        Ok(None)
    }
}
