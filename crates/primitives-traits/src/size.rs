pub use alloy_consensus::InMemorySize;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Header, TxEip1559, TxEip2930, TxEip4844, TxEip7702, TxLegacy};

    #[test]
    fn no_in_memory_no_recursion() {
        fn assert_no_recursion<T: InMemorySize + Default>() {
            let _ = T::default().size();
        }
        assert_no_recursion::<Header>();
        assert_no_recursion::<TxLegacy>();
        assert_no_recursion::<TxEip2930>();
        assert_no_recursion::<TxEip1559>();
        assert_no_recursion::<TxEip7702>();
        assert_no_recursion::<TxEip4844>();
    }
}
