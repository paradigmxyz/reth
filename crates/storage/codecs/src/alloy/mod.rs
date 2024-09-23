mod access_list;
mod authorization_list;
mod genesis_account;
mod header;
mod log;
mod request;
mod transaction;
mod trie;
mod txkind;
mod withdrawal;

#[cfg(test)]
mod tests {
    use crate::{
        alloy::{
            authorization_list::Authorization,
            genesis_account::{GenesisAccount, GenesisAccountRef},
            header::Header,
            transaction::{
                eip1559::TxEip1559, eip2930::TxEip2930, eip4844::TxEip4844, eip7702::TxEip7702,
                legacy::TxLegacy,
            },
            withdrawal::Withdrawal,
        },
        test_bitflag_unused_bits,
        test_utils::UnusedBits,
    };

    #[test]
    fn test_bitflag_unused_bits() {
        test_bitflag_unused_bits!(Header, UnusedBits::Zero);
        test_bitflag_unused_bits!(TxEip2930, UnusedBits::Zero);

        test_bitflag_unused_bits!(Authorization, UnusedBits::NotZero);
        test_bitflag_unused_bits!(GenesisAccountRef<'_>, UnusedBits::NotZero);
        test_bitflag_unused_bits!(GenesisAccount, UnusedBits::NotZero);
        test_bitflag_unused_bits!(TxEip1559, UnusedBits::NotZero);
        test_bitflag_unused_bits!(TxEip4844, UnusedBits::NotZero);
        test_bitflag_unused_bits!(TxEip7702, UnusedBits::NotZero);
        test_bitflag_unused_bits!(TxLegacy, UnusedBits::NotZero);
        test_bitflag_unused_bits!(Withdrawal, UnusedBits::NotZero);
    }
}
