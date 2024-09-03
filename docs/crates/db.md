# db

The database is a central component to Reth, enabling persistent storage for data like block headers, block bodies, transactions and more. The Reth database is comprised of key-value storage written to the disk and organized in tables. This chapter might feel a little dense at first, but shortly, you will feel very comfortable understanding and navigating the `db` crate. This chapter will go through the structure of the database, its tables and the mechanics of the `Database` trait.

<br>

## Tables

Within Reth, the database is organized via "tables". A table is any struct that implements the `Table` trait.

[File: crates/storage/db-api/src/table.rs](https://github.com/paradigmxyz/reth/blob/bf9cac7571f018fec581fe3647862dab527aeafb/crates/storage/db-api/src/table.rs#L64-L93)

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
pub trait Key: Encode + Decode + Ord + Clone + Serialize + for<'a> Deserialize<'a> {}

//--snip--
pub trait Value: Compress + Decompress + Serialize {}

```

The `Table` trait has two generic values, `Key` and `Value`, which need to implement the `Key` and `Value` traits, respectively. The `Encode` trait is responsible for transforming data into bytes so it can be stored in the database, while the `Decode` trait transforms the bytes back into its original form. Similarly, the `Compress` and `Decompress` traits transform the data to and from a compressed format when storing or reading data from the database.

There are many tables within the node, all used to store different types of data from `Headers` to `Transactions` and more. Below is a list of all of the tables. You can follow [this link](https://github.com/paradigmxyz/reth/blob/bf9cac7571f018fec581fe3647862dab527aeafb/crates/storage/db/src/tables/mod.rs#L274-L414) if you would like to see the table definitions for any of the tables below.

- CanonicalHeaders
- HeaderTerminalDifficulties
- HeaderNumbers
- Headers
- BlockBodyIndices
- BlockOmmers
- BlockWithdrawals
- Transactions
- TransactionHashNumbers
- TransactionBlocks
- Receipts
- Bytecodes
- PlainAccountState
- PlainStorageState
- AccountsHistory
- StoragesHistory
- AccountChangeSets
- StorageChangeSets
- HashedAccounts
- HashedStorages
- AccountsTrie
- StoragesTrie
- TransactionSenders
- StageCheckpoints
- StageCheckpointProgresses
- PruneCheckpoints
- VersionHistory
- BlockRequests
- ChainState

<br>

## Database

Reth's database design revolves around it's main [Database trait](https://github.com/paradigmxyz/reth/blob/bf9cac7571f018fec581fe3647862dab527aeafb/crates/storage/db-api/src/database.rs#L8-L52), which implements the database's functionality across many types. Let's take a quick look at the `Database` trait and how it works.

[File: crates/storage/db-api/src/database.rs](https://github.com/paradigmxyz/reth/blob/bf9cac7571f018fec581fe3647862dab527aeafb/crates/storage/db-api/src/database.rs#L8-L52)

```rust ignore
/// Main Database trait that can open read-only and read-write transactions.
///
/// Sealed trait which cannot be implemented by 3rd parties, exposed only for consumption.
pub trait Database: Send + Sync {
    /// Read-Only database transaction
    type TX: DbTx + Send + Sync + Debug + 'static;
    /// Read-Write database transaction
    type TXMut: DbTxMut + DbTx + TableImporter + Send + Sync + Debug + 'static;

    /// Create read only transaction.
    #[track_caller]
    fn tx(&self) -> Result<Self::TX, DatabaseError>;

    /// Create read write transaction only possible if database is open with write access.
    #[track_caller]
    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError>;

    /// Takes a function and passes a read-only transaction into it, making sure it's closed in the
    /// end of the execution.
    fn view<T, F>(&self, f: F) -> Result<T, DatabaseError>
    where
        F: FnOnce(&Self::TX) -> T,
    {
        let tx = self.tx()?;

        let res = f(&tx);
        tx.commit()?;

        Ok(res)
    }

    /// Takes a function and passes a write-read transaction into it, making sure it's committed in
    /// the end of the execution.
    fn update<T, F>(&self, f: F) -> Result<T, DatabaseError>
    where
        F: FnOnce(&Self::TXMut) -> T,
    {
        let tx = self.tx_mut()?;

        let res = f(&tx);
        tx.commit()?;

        Ok(res)
    }
}
```

Any type that implements the `Database` trait can create a database transaction, as well as view or update existing transactions. As an example, let's revisit the `Transaction` struct from the `stages` crate. This struct contains a field named `db` which is a reference to a generic type `DB` that implements the `Database` trait. The `Transaction` struct can use the `db` field to store new headers, bodies and senders in the database. In the code snippet below, you can see the `Transaction::open()` method, which uses the `Database::tx_mut()` function to create a mutable transaction.

[File: crates/stages/src/db.rs](https://github.com/paradigmxyz/reth/blob/00a49f5ee78b0a88fea409283e6bb9c96d4bb31e/crates/stages/src/db.rs#L28)

```rust ignore
pub struct Transaction<'this, DB: Database> {
    /// A handle to the DB.
    pub(crate) db: &'this DB,
    tx: Option<<DB as Database>::TXMut>,
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

The `Database` defines two associated types `TX` and `TXMut`.

[File: crates/storage/db-api/src/database.rs](https://github.com/paradigmxyz/reth/blob/bf9cac7571f018fec581fe3647862dab527aeafb/crates/storage/db-api/src/database.rs#L54-L78)

The `TX` type can be any type that implements the `DbTx` trait, which provides a set of functions to interact with read only transactions.

[File: crates/storage/db-api/src/transaction.rs](https://github.com/paradigmxyz/reth/blob/bf9cac7571f018fec581fe3647862dab527aeafb/crates/storage/db-api/src/transaction.rs#L7-L29)

```rust ignore
/// Read only transaction
pub trait DbTx: Send + Sync {
    /// Cursor type for this read-only transaction
    type Cursor<T: Table>: DbCursorRO<T> + Send + Sync;
    /// `DupCursor` type for this read-only transaction
    type DupCursor<T: DupSort>: DbDupCursorRO<T> + DbCursorRO<T> + Send + Sync;

    /// Get value
    fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, DatabaseError>;
    /// Commit for read only transaction will consume and free transaction and allows
    /// freeing of memory pages
    fn commit(self) -> Result<bool, DatabaseError>;
    /// Aborts transaction
    fn abort(self);
    /// Iterate over read only values in table.
    fn cursor_read<T: Table>(&self) -> Result<Self::Cursor<T>, DatabaseError>;
    /// Iterate over read only values in dup sorted table.
    fn cursor_dup_read<T: DupSort>(&self) -> Result<Self::DupCursor<T>, DatabaseError>;
    /// Returns number of entries in the table.
    fn entries<T: Table>(&self) -> Result<usize, DatabaseError>;
    /// Disables long-lived read transaction safety guarantees.
    fn disable_long_read_transaction_safety(&mut self);
}
```

The `TXMut` type can be any type that implements the `DbTxMut` trait, which provides a set of functions to interact with read/write transactions and the associated cursor types.

[File: crates/storage/db-api/src/transaction.rs](https://github.com/paradigmxyz/reth/blob/bf9cac7571f018fec581fe3647862dab527aeafb/crates/storage/db-api/src/transaction.rs#L31-L54)

```rust ignore
/// Read write transaction that allows writing to database
pub trait DbTxMut: Send + Sync {
    /// Read-Write Cursor type
    type CursorMut<T: Table>: DbCursorRW<T> + DbCursorRO<T> + Send + Sync;
    /// Read-Write `DupCursor` type
    type DupCursorMut<T: DupSort>: DbDupCursorRW<T>
        + DbCursorRW<T>
        + DbDupCursorRO<T>
        + DbCursorRO<T>
        + Send
        + Sync;

    /// Put value to database
    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), DatabaseError>;
    /// Delete value from database
    fn delete<T: Table>(&self, key: T::Key, value: Option<T::Value>)
        -> Result<bool, DatabaseError>;
    /// Clears database.
    fn clear<T: Table>(&self) -> Result<(), DatabaseError>;
    /// Cursor mut
    fn cursor_write<T: Table>(&self) -> Result<Self::CursorMut<T>, DatabaseError>;
    /// `DupCursor` mut.
    fn cursor_dup_write<T: DupSort>(&self) -> Result<Self::DupCursorMut<T>, DatabaseError>;
}
```

Let's take a look at the `DbTx` and `DbTxMut` traits in action.

Revisiting the `DatabaseProvider<Tx>` struct as an exampl, the `DatabaseProvider<Tx>::header_by_number()` function uses the `DbTx::get()` function to get a header from the `Headers` table.

[File: crates/storage/provider/src/providers/database/provider.rs](https://github.com/paradigmxyz/reth/blob/bf9cac7571f018fec581fe3647862dab527aeafb/crates/storage/provider/src/providers/database/provider.rs#L1319-L1336)

```rust ignore
impl<TX: DbTx> HeaderProvider for DatabaseProvider<TX> {
   //--snip--

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Header>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            num,
            |static_file| static_file.header_by_number(num),
            || Ok(self.tx.get::<tables::Headers>(num)?),
        )
    }

   //--snip--
}
```

Notice that the function uses a [turbofish](https://techblog.tonsser.com/posts/what-is-rusts-turbofish) to define which table to use when passing in the `key` to the `DbTx::get()` function. Taking a quick look at the function definition, a generic `T` is defined that implements the `Table` trait mentioned at the beginning of this chapter.

[File: crates/storage/db-api/src/transaction.rs](https://github.com/paradigmxyz/reth/blob/bf9cac7571f018fec581fe3647862dab527aeafb/crates/storage/db-api/src/transaction.rs#L15)

```rust ignore
fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, DatabaseError>;
```

This design pattern is very powerful and allows Reth to use the methods available to the `DbTx` and `DbTxMut` traits without having to define implementation blocks for each table within the database.

Let's take a look at a couple examples before moving on. In the snippet below, the `DbTxMut::put()` method is used to insert values into the `CanonicalHeaders`, `Headers` and `HeaderNumbers` tables.

[File: crates/storage/provider/src/providers/database/provider.rs](https://github.com/paradigmxyz/reth/blob/bf9cac7571f018fec581fe3647862dab527aeafb/crates/storage/provider/src/providers/database/provider.rs#L2606-L2745)

```rust ignore
self.tx.put::<tables::CanonicalHeaders>(block_number, block.hash())?;
self.tx.put::<tables::Headers>(block_number, block.header.as_ref().clone())?;
self.tx.put::<tables::HeaderNumbers>(block.hash(), block_number)?;
```

Let's take a look at the `DatabaseProviderRW<DB: Database>` struct, which is used to create a mutable transaction to interact with the database.
The `DatabaseProviderRW<DB: Database>` struct implements the `Deref` and `DerefMut` trait, which returns a reference to its first field, which is a `TxMut`. Recall that `TxMut` is a generic type on the `Database` trait, which is defined as `type TXMut: DbTxMut + DbTx + Send + Sync;`, giving it access to all of the functions available to `DbTx`, including the `DbTx::get()` function.

This next example uses the `DbTx::cursor()` method to get a `Cursor`. The `Cursor` type provides a way to traverse through rows in a database table, one row at a time. A cursor enables the program to perform an operation (updating, deleting, etc) on each row in the table individually. The following code snippet gets a cursor for a few different tables in the database.

[File: crates/static-file/static-file/src/segments/headers.rs](https://github.com/paradigmxyz/reth/blob/bf9cac7571f018fec581fe3647862dab527aeafb/crates/static-file/static-file/src/segments/headers.rs#L22-L58)

```rust ignore
# Get a cursor for the Headers table
let mut headers_cursor = provider.tx_ref().cursor_read::<tables::Headers>()?;
# Then we can walk the cursor to get the headers for a specific block range
let headers_walker = headers_cursor.walk_range(block_range.clone())?;
```

Lets look at an examples of how cursors are used. The code snippet below contains the `unwind` method from the `BodyStage` defined in the `stages` crate. This function is responsible for unwinding any changes to the database if there is an error when executing the body stage within the Reth pipeline.

[File: crates/stages/stages/src/stages/bodies.rs](https://github.com/paradigmxyz/reth/blob/bf9cac7571f018fec581fe3647862dab527aeafb/crates/stages/stages/src/stages/bodies.rs#L267-L345)

```rust ignore
/// Unwind the stage.
fn unwind(&mut self, provider: &DatabaseProviderRW<DB>, input: UnwindInput) {
    self.buffer.take();

    let static_file_provider = provider.static_file_provider();
    let tx = provider.tx_ref();
    // Cursors to unwind bodies, ommers
    let mut body_cursor = tx.cursor_write::<tables::BlockBodyIndices>()?;
    let mut ommers_cursor = tx.cursor_write::<tables::BlockOmmers>()?;
    let mut withdrawals_cursor = tx.cursor_write::<tables::BlockWithdrawals>()?;
    let mut requests_cursor = tx.cursor_write::<tables::BlockRequests>()?;
    // Cursors to unwind transitions
    let mut tx_block_cursor = tx.cursor_write::<tables::TransactionBlocks>()?;

    let mut rev_walker = body_cursor.walk_back(None)?;
    while let Some((number, block_meta)) = rev_walker.next().transpose()? {
        if number <= input.unwind_to {
            break
        }

        // Delete the ommers entry if any
        if ommers_cursor.seek_exact(number)?.is_some() {
            ommers_cursor.delete_current()?;
        }

        // Delete the withdrawals entry if any
        if withdrawals_cursor.seek_exact(number)?.is_some() {
            withdrawals_cursor.delete_current()?;
        }

        // Delete the requests entry if any
        if requests_cursor.seek_exact(number)?.is_some() {
            requests_cursor.delete_current()?;
        }

        // Delete all transaction to block values.
        if !block_meta.is_empty() &&
            tx_block_cursor.seek_exact(block_meta.last_tx_num())?.is_some()
        {
            tx_block_cursor.delete_current()?;
        }

        // Delete the current body value
        rev_walker.delete_current()?;
    }
    //--snip--
}
```

This function first grabs a mutable cursor for the `BlockBodyIndices`, `BlockOmmers`, `BlockWithdrawals`, `BlockRequests`, `TransactionBlocks` tables.

Then it gets a walker of the block body cursor, and then walk backwards through the cursor to delete the block body entries from the last block number to the block number specified in the `UnwindInput` struct.

While this is a brief look at how cursors work in the context of database tables, the chapter on the `libmdbx` crate will go into further detail on how cursors communicate with the database and what is actually happening under the hood.

<br>

## Summary

This chapter was packed with information, so lets do a quick review. The database is comprised of tables, with each table being a collection of key-value pairs representing various pieces of data in the blockchain. Any struct that implements the `Database` trait can view, update or delete entries in the various tables. The database design leverages nested traits and generic associated types to provide methods to interact with each table in the database.

<br>

# Next Chapter

[Next Chapter]()
