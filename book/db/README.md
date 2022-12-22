# db

The database is a central component to Reth, enabling persistent storage for data like block headers, block bodies, transactions and more. The Reth database is comprised of key-value storage written to the disk and organized in tables. Within Reth, a table is a struct that implements the `Table` trait.

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


Reth's database design revolves around it's main [Database trait](https://github.com/paradigmxyz/reth/blob/0d9b9a392d4196793736522f3fc2ac804991b45d/crates/interfaces/src/db/mod.rs#L33), which takes advantage of [generic associated types]() to implement the database's functionality across many types. Let's take a quick look at the `Database` trait and how it works. This section might feel a little dense at first, but shortly, you will feel very comfortable with Reth's use of GATs and the `Database` trait in its implementation. 

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
In the code snippet above is the `Database` trait. Any type that implements the `Database` trait can create a read only transaction, create a read/write transaction, view an existing transaction or update an existing transaction. We already saw one type that implements the `Database` trait, which is the `StageDB` from the `stages` chapter of the book. `StageDB` uses the `Database` trait to store new headers, bodies and senders in the database during the loop in the Reth  pipeline. In the code snippet below, you can see the `StageDB` struct as well as its `open()` function which uses the `Database::tx_mut()` function to create a mutable transaction. 

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

If you keep looking through the `StageDB` impl, we will start to find a few methods that are used on `self`, but are not defined within the `Database` trait. For example, the `get_block_hash()` method, uses `self.get()` which is not defined within the `Database` trait.

[File: ]()
```rust ignore
impl<'this, DB> StageDB<'this, DB>
where
    DB: Database,
{\
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


While is not immediately known where the `self.get()` function is coming from, the answer lies one step deeper into the `Database` trait. Lets take a deeper look at the `Database` trait, looking at the `DatabaseGAT` trait and its associated types. Revising the `Database` trait's definition, we can see that the trait also implements the `DatabaseGAT` trait.

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

In Rust, traits can have associated types, which are like generics, but they are associated with a trait and can only be used in the context of that trait. 

In the code snippet above, the `DatabaseGAT` trait has two associated types, `TX` and `TXMut`, and requires the `Send` and `Sync` traits to be implemented for all types that are used for `TX` and `TXMut`. The `TX` type gives any type that implements the `DatabaseGAT` trait, read only access to the database, while the `TXMut` give read and write access as well. The `Database` trait implements the `DatabaseGAT` trait, meaning that any type that implements the `Database` trait must also implement the `DatabaseGAT` trait. In the same way that the `Database` trait implements the `DatabaseGAT` trait, giving it access to its functions, the `TX` and `TXMut` types implement the `DbTx` and `DbTxMut` traits, respectively. 

[File: ]()
```rust ignore

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

Lets take a look at the `DbTx` and `DbTxMut` traits in action. Below is the the implementation block for the `StageId` struct used throughout the `stages` crate. This struct is defined as `pub struct StageId(pub &'static str);` and simply holds an Id for a given stage in the Reth pipeline. Within the `StageId` implementation block, you can see a few variables that the implement the `DbTx` or `DbTxMut`.

[File: ]()
```rust ignore

impl StageId {
    /// Get the last committed progress of this stage.
    pub fn get_progress<'db>(&self, tx: &impl DbTx<'db>) -> Result<Option<BlockNumber>, DbError> {
        tx.get::<SyncStage>(self.0.as_bytes().to_vec())
    }

    /// Save the progress of this stage.
    pub fn save_progress<'db>(
        &self,
        tx: &impl DbTxMut<'db>,
        block: BlockNumber,
    ) -> Result<(), DbError> {
        counter!("stage.progress", block, "stage" => self.0);
        tx.put::<SyncStage>(self.0.as_bytes().to_vec(), block)
    }
}
```

Within the `get_process` function, the `tx` variable uses the `get()` method, which is defined in the `DbTx` trait.

```rust ignore
pub trait DbTx<'tx>: for<'a> DbTxGAT<'a> {
    fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, Error>;
//--snip--
}
```

This method defines a generic type `T` that implements the `Table` trait, and takes a `key` variable that is the 

Within the `StageId::get_progress()` function, the `get` method uses the [turbofish operator]() to define which type to use that implements the `Table` trait, using the following syntax: `tx.get::<SyncStage>(self.0.as_bytes().to_vec())`. 


Similarly, within the `save_progress` method, the tx variable uses the `put()` method, which is defined in th `DbTxMut` trait. The `put` method within `StageId::save_progress()` also uses the turbofish operator to define which table to use, placing values into the table.

```rust ignore 
    pub fn save_progress<'db>(
        &self,
        tx: &impl DbTxMut<'db>,
        block: BlockNumber,
    ) -> Result<(), DbError> {
        
        //--snip--
        tx.put::<SyncStage>(self.0.as_bytes().to_vec(), block)
    }
```



//TODO: Explore DbTxGAT, Cursor and DupCursor

//TODO: Make a note about how this chapter covers the database and the related traits until a certain point, where the libmdbx section will cover cursors in more detail.


<!-- 

[File: ]()
```rust ignore

/// Read only Cursor.
pub type CursorRO<'tx, T> = Cursor<'tx, RO, T>;
/// Read write cursor.
pub type CursorRW<'tx, T> = Cursor<'tx, RW, T>;

/// Cursor wrapper to access KV items.
#[derive(Debug)]
pub struct Cursor<'tx, K: TransactionKind, T: Table> {
    /// Inner `libmdbx` cursor.
    pub inner: reth_libmdbx::Cursor<'tx, K>,
    /// Table name as is inside the database.
    pub table: &'static str,
    /// Phantom data to enforce encoding/decoding.
    pub _dbi: std::marker::PhantomData<T>,
}
``` -->



Ok, lets do a quick review.
<!-- Insert a diagram of the traits maybe?-->
TODO: Very digestible overview of how the db trait works as a whole in a few sentences.