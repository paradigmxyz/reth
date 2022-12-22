# db

The database is a central component to Reth, enabling persistent storage for data like block headers, block bodies, transactions and more. The Reth database is comprised of key-value storage written to the disk and organized in tables. This chapter might feel a little dense at first, but shortly, you will feel very comfortable understanding and navigating the `db` crate. This chapter will go through the structure of the database, its tables and the mechanics of the `Database` trait.

## Tables

Within Reth, a table is a struct that implements the `Table` trait.

```rust ignore
pub trait Table: Send + Sync + Debug + 'static {
    /// Return table name as it is present inside the MDBX.
    const NAME: &'static str;
    /// Key element of `Table`.
    ///
    /// Sorting should be taken into account when encoding this.
    type Key: Key;
    /// Value element of `Table`.
    type Value: Value;
}

//--snip--
pub trait Key: Encode + Decode {}

//--snip--
pub trait Value: Compress + Decompress {}

```

The `Table` trait has two generic values, `Key` and `Value`, which need to implement the `Key` and `Value` traits, respectively. There are many tables within the node, all used to store different types of data from `Headers` to `Transactions` and more. Below is a list of all of the tables. You can follow [this link]() if you would like to see the table definitions for any of the tables below.

- CanonicalHeaders
- HeaderTD
- HeaderNumbers
- Headers
- CumulativeTxCount
- NonCanonicalTransactions
- Transactions
- Receipts
- Logs
- PlainAccountState
- PlainStorageState
- Bytecodes
- AccountHistory
- AccountHistory
- StorageHistory
- AccountChangeSet
- StorageChangeSet
- TxSenders
- Config
- SyncStage


## Database

Reth's database design revolves around it's main [Database trait](https://github.com/paradigmxyz/reth/blob/0d9b9a392d4196793736522f3fc2ac804991b45d/crates/interfaces/src/db/mod.rs#L33), which takes advantage of [generic associated types]() to implement the database's functionality across many types. Let's take a quick look at the `Database` trait and how it works.

```rust ignore
/// Main Database trait that spawns transactions to be executed.
pub trait Database: for<'a> DatabaseGAT<'a> {
    /// Create read only transaction.
    fn tx(&self) -> Result<<Self as DatabaseGAT<'_>>::TX, Error>;

    /// Create read write transaction only possible if database is open with write access.
    fn tx_mut(&self) -> Result<<Self as DatabaseGAT<'_>>::TXMut, Error>;

    /// Takes a function and passes a read-only transaction into it, making sure it's closed in the
    /// end of the execution.
    fn view<T, F>(&self, f: F) -> Result<T, Error>
    where
        F: Fn(&<Self as DatabaseGAT<'_>>::TX) -> T,
    {
        let tx = self.tx()?;

        let res = f(&tx);
        tx.commit()?;

        Ok(res)
    }

    /// Takes a function and passes a write-read transaction into it, making sure it's committed in
    /// the end of the execution.
    fn update<T, F>(&self, f: F) -> Result<T, Error>
    where
        F: Fn(&<Self as DatabaseGAT<'_>>::TXMut) -> T,
    {
        let tx = self.tx_mut()?;

        let res = f(&tx);
        tx.commit()?;

        Ok(res)
    }
}
```

<!-- TODO: give some examples of what a transaction could actually be -->
The code snippet above contains the `Database` trait. Any type that implements the `Database` trait can create a database transaction, view an existing transaction or update an existing transaction. We already saw that `StageDB` from the `stages` chapter of the book implements the `Database` trait, allowing it to store new headers, bodies and senders in the database during the Reth pipeline loop. As an example, in the code snippet below, you can see the `open()` method defined on the `StageDB` struct, which uses the `Database::tx_mut()` function to create a mutable transaction. 

[File: ]()
```rust ignore
pub struct StageDB<'this, DB: Database> {
    /// A handle to the DB.
    pub(crate) db: &'this DB,
    tx: Option<<DB as DatabaseGAT<'this>>::TXMut>,
}

//--snip--
impl<'this, DB> StageDB<'this, DB>
where
    DB: Database,
{
    //--snip--

    /// Open a new inner transaction.
    pub fn open(&mut self) -> Result<(), Error> {
        self.tx = Some(self.db.tx_mut()?);
        Ok(())
    }
}
```

The `Database` trait also implements the `DatabaseGAT` trait which defines two associated types `TX` and `TXMut`.

[File: ]()
```rust ignore
/// Implements the GAT method from:
/// https://sabrinajewson.org/blog/the-better-alternative-to-lifetime-gats#the-better-gats.
///
/// Sealed trait which cannot be implemented by 3rd parties, exposed only for implementers
pub trait DatabaseGAT<'a, __ImplicitBounds: Sealed = Bounds<&'a Self>>: Send + Sync {
    /// RO database transaction
    type TX: DbTx<'a> + Send + Sync;
    /// RW database transaction
    type TXMut: DbTxMut<'a> + DbTx<'a> + Send + Sync;
}
```

In Rust, associated types are like generics in that they can be any type fitting the generic's definition, but associated types are associated with a trait and can only be used in the context of that trait. 

In the code snippet above, the `DatabaseGAT` trait has two associated types, `TX` and `TXMut`. 

The `TX` type can be any type that implements the `DbTx` trait, which provides a set of functions to interact with read only transactions. 

[File: ]()
```rust ignore
/// Read only transaction
pub trait DbTx<'tx>: for<'a> DbTxGAT<'a> {
    /// Get value
    fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, Error>;
    /// Commit for read only transaction will consume and free transaction and allows
    /// freeing of memory pages
    fn commit(self) -> Result<bool, Error>;
    /// Iterate over read only values in table.
    fn cursor<T: Table>(&self) -> Result<<Self as DbTxGAT<'_>>::Cursor<T>, Error>;
    /// Iterate over read only values in dup sorted table.
    fn cursor_dup<T: DupSort>(&self) -> Result<<Self as DbTxGAT<'_>>::DupCursor<T>, Error>;
}
```

The `TXMut` type can be any type that implements the `DbTxMut` trait, which provides a set of functions to interact with read/write transactions.

[File: ]()
```rust ignore
/// Read write transaction that allows writing to database
pub trait DbTxMut<'tx>: for<'a> DbTxMutGAT<'a> {
    /// Put value to database
    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), Error>;
    /// Delete value from database
    fn delete<T: Table>(&self, key: T::Key, value: Option<T::Value>) -> Result<bool, Error>;
    /// Clears database.
    fn clear<T: Table>(&self) -> Result<(), Error>;
    /// Cursor mut
    fn cursor_mut<T: Table>(&self) -> Result<<Self as DbTxMutGAT<'_>>::CursorMut<T>, Error>;
    /// DupCursor mut.
    fn cursor_dup_mut<T: DupSort>(
        &self,
    ) -> Result<<Self as DbTxMutGAT<'_>>::DupCursorMut<T>, Error>;
}
```

Lets take a look at the `DbTx` and `DbTxMut` traits in action. Revisiting the `StageDB` as an example, the `get_block_hash` method uses the `get()` method defined in the `DbTx` trait. Remember that the `StageDB` struct implements the `Database` trait, which implements the `DatabaseGAT` trait, which defines the `TX` type that implements the `DbTx` trait.

[File: ]()
```rust ignore

impl<'this, DB> StageDB<'this, DB>
where
    DB: Database,
{
   //--snip--

    /// Query [tables::CanonicalHeaders] table for block hash by block number
    pub(crate) fn get_block_hash(&self, number: BlockNumber) -> Result<BlockHash, StageError> {
        let hash = self
            .get::<tables::CanonicalHeaders>(number)?
            .ok_or(DatabaseIntegrityError::CanonicalHash { number })?;
        Ok(hash)
    }
   //--snip--
}
```

The `get_block_hash()` method takes a `BlockNumber` and returns the block hash at that block number. To do this, the function uses `self.get::<tables::CanonicalHeaders>(number)?.ok_or(DatabaseIntegrityError::CanonicalHash { number })?;`.

Notice that function uses the the [turbofish operator]() to define which table to use when passing in the `key` to the `DbTx::get()` method. This is possible because the `DbTx::get()` function defines a generic `T` that implements the `Table` trait mentioned at the beginning of this chapter. 

[File: ]()
```rust ignore
fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, Error>;
```

This design pattern is very powerful and allows Reth to use the methods available to the `DbTx` and `DbTxMut` traits without having to define implementation blocks for each table within the database.  

Lets take a look at a few examples before moving on. In the snippet below, the `DbTx::get()` method is used to get the transaction number from the `CumulativeTxCount` table.

[File: ]()
```rust ignore
let transaction_number = tx
    .get::<tables::CumulativeTxCount>(block_num_hash.into())?
    .ok_or(Error::BlockTxNumberNotExists { block_hash })?;
```

This next example uses the `DbTxMut::put()` method to insert values into the `CanonicalHeaders`, `Headers` and `HeaderNumbers` tables.

[File: ]()
```rust ignore 
    let block_num_hash = BlockNumHash((block.number, block.hash()));
    tx.put::<tables::CanonicalHeaders>(block.number, block.hash())?;
    // Put header with canonical hashes.
    tx.put::<tables::Headers>(block_num_hash, block.header.as_ref().clone())?;
    tx.put::<tables::HeaderNumbers>(block.hash(), block.number)?;
```


This last example uses the `DbTx::cursor()` method to get a `Cursor`. The [`Cursor`]() type provides is a way to traverse through rows in a database table, one row at a time. It allows you to perform an operation on each row in the table individually, like updating, an existing value. The following code snippet gets a cursor for a few different tables in the database.

[File: ]()
```rust ignore 
// Get next canonical block hashes to execute.
    let mut canonicals = db_tx.cursor::<tables::CanonicalHeaders>()?;
    // Get header with canonical hashes.
    let mut headers = db_tx.cursor::<tables::Headers>()?;
    // Get bodies (to get tx index) with canonical hashes.
    let mut cumulative_tx_count = db_tx.cursor::<tables::CumulativeTxCount>()?;
    // Get transaction of the block that we are executing.
    let mut tx = db_tx.cursor::<tables::Transactions>()?;
    // Skip sender recovery and load signer from database.
    let mut tx_sender = db_tx.cursor::<tables::TxSenders>()?;

```


We are almost at the last stop in this tour of the `db` crate. In addition to the methods provided by the `DbTx` and `DbTxMut` traits, `DbTx` also inherits the `DbTxGAT` trait, while `DbTxMut` inherits `DbTxMutGAT`. This next level of inherited traits provides various associated types related to cursors as well as methods to utilize the cursor types.

[File: ]()
```rust ignore
pub trait DbTxGAT<'a, __ImplicitBounds: Sealed = Bounds<&'a Self>>: Send + Sync {
    /// Cursor GAT
    type Cursor<T: Table>: DbCursorRO<'a, T> + Send + Sync;
    /// DupCursor GAT
    type DupCursor<T: DupSort>: DbDupCursorRO<'a, T> + DbCursorRO<'a, T> + Send + Sync;
}
```



//TODO: Make a note about how this chapter covers the database and the related traits until a certain point, where the libmdbx section will cover cursors in more detail.



Ok, lets do a quick review.
<!-- Insert a diagram of the traits maybe?-->
TODO: Very digestible overview of how the db trait works as a whole in a few sentences.