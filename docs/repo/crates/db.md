# db

The database is a central component to Reth, enabling persistent storage for data like block headers, block bodies, transactions and more. The Reth database is comprised of key-value storage written to the disk and organized in tables. This chapter might feel a little dense at first, but shortly, you will feel very comfortable understanding and navigating the `db` crate. This chapter will go through the structure of the database, its tables and the mechanics of the `Database` trait.

<br>

## Tables

Within Reth, the database is organized via "tables". A table is any struct that implements the `Table` trait.

[File: crates/storage/db/src/abstraction/table.rs](https://github.com/paradigmxyz/reth/blob/main/crates/storage/db/src/abstraction/table.rs#L56-L65)

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
pub trait Key: Encode + Decode + Ord {}

//--snip--
pub trait Value: Compress + Decompress + Serialize {}

```

The `Table` trait has two generic values, `Key` and `Value`, which need to implement the `Key` and `Value` traits, respectively. The `Encode` trait is responsible for transforming data into bytes so it can be stored in the database, while the `Decode` trait transforms the bytes back into its original form. Similarly, the `Compress` and `Decompress` traits transform the data to and from a compressed format when storing or reading data from the database.

There are many tables within the node, all used to store different types of data from `Headers` to `Transactions` and more. Below is a list of all of the tables. You can follow [this link](https://github.com/paradigmxyz/reth/blob/main/crates/storage/db/src/tables/mod.rs#L36) if you would like to see the table definitions for any of the tables below.

- CanonicalHeaders
- HeaderTD
- HeaderNumbers
- Headers
- BlockBodies
- BlockOmmers
- Transactions
- TxHashNumber
- Receipts
- Logs
- PlainAccountState
- PlainStorageState
- Bytecodes
- BlockTransitionIndex
- TxTransitionIndex
- AccountHistory
- StorageHistory
- AccountChangeSet
- StorageChangeSet
- TxSenders
- Config
- SyncStage

<br>

## Database

Reth's database design revolves around it's main [Database trait](https://github.com/paradigmxyz/reth/blob/0d9b9a392d4196793736522f3fc2ac804991b45d/crates/interfaces/src/db/mod.rs#L33), which takes advantage of [generic associated types](https://blog.rust-lang.org/2022/10/28/gats-stabilization.html) and [a few design tricks](https://sabrinajewson.org/blog/the-better-alternative-to-lifetime-gats#the-better-gats) to implement the database's functionality across many types. Let's take a quick look at the `Database` trait and how it works.

[File: crates/storage/db/src/abstraction/database.rs](https://github.com/paradigmxyz/reth/blob/main/crates/storage/db/src/abstraction/database.rs#L19)

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

Any type that implements the `Database` trait can create a database transaction, as well as view or update existing transactions. As an example, lets revisit the `Transaction` struct from the `stages` crate. This struct contains a field named `db` which is a reference to a generic type `DB` that implements the `Database` trait. The `Transaction` struct can use the `db` field to store new headers, bodies and senders in the database. In the code snippet below, you can see the `Transaction::open()` method, which uses the `Database::tx_mut()` function to create a mutable transaction.

[File: crates/stages/src/db.rs](https://github.com/paradigmxyz/reth/blob/main/crates/stages/src/db.rs#L95-L98)

```rust ignore
pub struct Transaction<'this, DB: Database> {
    /// A handle to the DB.
    pub(crate) db: &'this DB,
    tx: Option<<DB as DatabaseGAT<'this>>::TXMut>,
}

//--snip--
impl<'this, DB> Transaction<'this, DB>
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

[File: crates/storage/db/src/abstraction/database.rs](https://github.com/paradigmxyz/reth/blob/main/crates/storage/db/src/abstraction/database.rs#L11)

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

In Rust, associated types are like generics in that they can be any type fitting the generic's definition, with the difference being that associated types are associated with a trait and can only be used in the context of that trait.

In the code snippet above, the `DatabaseGAT` trait has two associated types, `TX` and `TXMut`.

The `TX` type can be any type that implements the `DbTx` trait, which provides a set of functions to interact with read only transactions.

[File: crates/storage/db/src/abstraction/transaction.rs](https://github.com/paradigmxyz/reth/blob/main/crates/storage/db/src/abstraction/transaction.rs#L36)

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

[File: crates/storage/db/src/abstraction/transaction.rs](https://github.com/paradigmxyz/reth/blob/main/crates/storage/db/src/abstraction/transaction.rs#L49)

```rust ignore
/// Read write transaction that allows writing to database
pub trait DbTxMut<'tx>: for<'a> DbTxMutGAT<'a> {
    /// Put value to database
    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), Error>;
    /// Delete value from database
    fn delete<T: Table>(&self, key: T::Key, value: Option<T::Value>) -> Result<bool, Error>;
    /// Clears database.
    fn clear<T: Table>(&self) -> Result<(), Error>;
    /// Cursor for writing
    fn cursor_write<T: Table>(&self) -> Result<<Self as DbTxMutGAT<'_>>::CursorMut<T>, Error>;
    /// DupCursor for writing
    fn cursor_dup_write<T: DupSort>(
        &self,
    ) -> Result<<Self as DbTxMutGAT<'_>>::DupCursorMut<T>, Error>;
}
```

Lets take a look at the `DbTx` and `DbTxMut` traits in action. Revisiting the `Transaction` struct as an example, the `Transaction::get_block_hash()` method uses the `DbTx::get()` function to get a block header hash in the form of `self.get::<tables::CanonicalHeaders>(number)`.

[File: crates/storage/provider/src/transaction.rs](https://github.com/paradigmxyz/reth/blob/main/crates/storage/provider/src/transaction.rs#L106)

```rust ignore

impl<'this, DB> Transaction<'this, DB>
where
    DB: Database,
{
   //--snip--

    /// Query [tables::CanonicalHeaders] table for block hash by block number
    pub(crate) fn get_block_hash(&self, number: BlockNumber) -> Result<BlockHash, StageError> {
        let hash = self
            .get::<tables::CanonicalHeaders>(number)?
            .ok_or(ProviderError::CanonicalHash { number })?;
        Ok(hash)
    }
   //--snip--
}

//--snip--
impl<'a, DB: Database> Deref for Transaction<'a, DB> {
    type Target = <DB as DatabaseGAT<'a>>::TXMut;
    fn deref(&self) -> &Self::Target {
        self.tx.as_ref().expect("Tried getting a reference to a non-existent transaction")
    }
}
```

The `Transaction` struct implements the `Deref` trait, which returns a reference to its `tx` field, which is a `TxMut`. Recall that `TxMut` is a generic type on the `DatabaseGAT` trait, which is defined as `type TXMut: DbTxMut<'a> + DbTx<'a> + Send + Sync;`, giving it access to all of the functions available to `DbTx`, including the `DbTx::get()` function.

Notice that the function uses a [turbofish](https://techblog.tonsser.com/posts/what-is-rusts-turbofish) to define which table to use when passing in the `key` to the `DbTx::get()` function. Taking a quick look at the function definition, a generic `T` is defined that implements the `Table` trait mentioned at the beginning of this chapter.

[File: crates/storage/db/src/abstraction/transaction.rs](https://github.com/paradigmxyz/reth/blob/main/crates/storage/db/src/abstraction/transaction.rs#L38)

```rust ignore
fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, Error>;
```

This design pattern is very powerful and allows Reth to use the methods available to the `DbTx` and `DbTxMut` traits without having to define implementation blocks for each table within the database.

Lets take a look at a couple examples before moving on. In the snippet below, the `DbTxMut::put()` method is used to insert values into the `CanonicalHeaders`, `Headers` and `HeaderNumbers` tables.

[File: crates/storage/provider/src/block.rs](https://github.com/paradigmxyz/reth/blob/main/crates/storage/provider/src/block.rs#L121-L125)

```rust ignore
    tx.put::<tables::CanonicalHeaders>(block.number, block.hash())?;
    // Put header with canonical hashes.
    tx.put::<tables::Headers>(block.number, block.header.as_ref().clone())?;
    tx.put::<tables::HeaderNumbers>(block.hash(), block.number)?;
```

This next example uses the `DbTx::cursor()` method to get a `Cursor`. The `Cursor` type provides a way to traverse through rows in a database table, one row at a time. A cursor enables the program to perform an operation (updating, deleting, etc) on each row in the table individually. The following code snippet gets a cursor for a few different tables in the database.

[File: crates/stages/src/stages/execution.rs](https://github.com/paradigmxyz/reth/blob/main/crates/stages/src/stages/execution.rs#L93-L101)

```rust ignore
// Get next canonical block hashes to execute.
    let mut canonicals = db_tx.cursor_read::<tables::CanonicalHeaders>()?;
    // Get header with canonical hashes.
    let mut headers = db_tx.cursor_read::<tables::Headers>()?;
    // Get bodies (to get tx index) with canonical hashes.
    let mut cumulative_tx_count = db_tx.cursor_read::<tables::CumulativeTxCount>()?;
    // Get transaction of the block that we are executing.
    let mut tx = db_tx.cursor_read::<tables::Transactions>()?;
    // Skip sender recovery and load signer from database.
    let mut tx_sender = db_tx.cursor_read::<tables::TxSenders>()?;

```

We are almost at the last stop in the tour of the `db` crate. In addition to the methods provided by the `DbTx` and `DbTxMut` traits, `DbTx` also inherits the `DbTxGAT` trait, while `DbTxMut` inherits `DbTxMutGAT`. These next two traits provide various associated types related to cursors as well as methods to utilize the cursor types.

[File: crates/storage/db/src/abstraction/transaction.rs](https://github.com/paradigmxyz/reth/blob/main/crates/storage/db/src/abstraction/transaction.rs#L12-L17)

```rust ignore
pub trait DbTxGAT<'a, __ImplicitBounds: Sealed = Bounds<&'a Self>>: Send + Sync {
    /// Cursor GAT
    type Cursor<T: Table>: DbCursorRO<'a, T> + Send + Sync;
    /// DupCursor GAT
    type DupCursor<T: DupSort>: DbDupCursorRO<'a, T> + DbCursorRO<'a, T> + Send + Sync;
}
```

Lets look at an examples of how cursors are used. The code snippet below contains the `unwind` method from the `BodyStage` defined in the `stages` crate. This function is responsible for unwinding any changes to the database if there is an error when executing the body stage within the Reth pipeline.

[File: crates/stages/src/stages/bodies.rs](https://github.com/paradigmxyz/reth/blob/main/crates/stages/src/stages/bodies.rs#L205-L238)

```rust ignore
 /// Unwind the stage.
    async fn unwind(
        &mut self,
        db: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        let mut tx_count_cursor = db.cursor_write::<tables::CumulativeTxCount>()?;
        let mut block_ommers_cursor = db.cursor_write::<tables::BlockOmmers>()?;
        let mut transaction_cursor = db.cursor_write::<tables::Transactions>()?;

        let mut entry = tx_count_cursor.last()?;
        while let Some((key, count)) = entry {
            if key.number() <= input.unwind_to {
                break
            }

            tx_count_cursor.delete_current()?;
            entry = tx_count_cursor.prev()?;

            if block_ommers_cursor.seek_exact(key)?.is_some() {
                block_ommers_cursor.delete_current()?;
            }

            let prev_count = entry.map(|(_, v)| v).unwrap_or_default();
            for tx_id in prev_count..count {
                if transaction_cursor.seek_exact(tx_id)?.is_some() {
                    transaction_cursor.delete_current()?;
                }
            }
        }

    //--snip--
    }

```

This function first grabs a mutable cursor for the `CumulativeTxCount`, `BlockOmmers` and `Transactions` tables.

The `tx_count_cursor` is used to get the last key value pair written to the `CumulativeTxCount` table and delete key value pair where the cursor is currently pointing.

The `block_ommers_cursor` is used to get the block ommers from the `BlockOmmers` table at the specified key, and delete the entry where the cursor is currently pointing.

Finally, the `transaction_cursor` is used to get delete each transaction from the last `TXNumber` written to the database, to the current tx count.

While this is a brief look at how cursors work in the context of database tables, the chapter on the `libmdbx` crate will go into further detail on how cursors communicate with the database and what is actually happening under the hood.

<br>

## Summary

This chapter was packed with information, so lets do a quick review. The database is comprised of tables, with each table being a collection of key-value pairs representing various pieces of data in the blockchain. Any struct that implements the `Database` trait can view, update or delete entries in the various tables. The database design leverages nested traits and generic associated types to provide methods to interact with each table in the database.

<br>

# Next Chapter

[Next Chapter]()
