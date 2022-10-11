mod error;
mod table;


pub use error::KVError;
pub use table::*;


pub trait Database<T: Table> {
    fn tx<'a>(&'a self) -> Box<dyn DbTx<'a, T> + 'a>;

    fn tx_mut<'a>(&'a self) -> Box<dyn DbTxMut<'a, T> + 'a>;
}

pub trait DbTx<'a, T: Table> {
    fn commit(self);
    //fn cursor(&'a self) -> Cursor<'a, RO, T>;
    fn get(&self) -> Option<T::Value>;
}

pub trait DbTxMut<'a, T: Table>: DbTx<'a, T> {
    fn put(&self);
    fn delete(&self);
    //fn cursor_mut(&self) -> Cursor<'a, RW, T>;
}
