#![feature(prelude_import)]
#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(
    test(
        no_crate_inject,
        attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
    )
)]
//! Commonly used types in reth.
#[prelude_import]
use std::prelude::rust_2021::*;
#[macro_use]
extern crate std;
mod account {
    use crate::{H256, U256};
    use codecs::main_codec;
    /// Account saved in database
    pub struct Account {
        /// Nonce.
        #[codec(compact)]
        pub nonce: u64,
        /// Account balance.
        pub balance: U256,
        /// Hash of the bytecode.
        pub bytecode_hash: H256,
    }
    #[automatically_derived]
    impl ::core::clone::Clone for Account {
        #[inline]
        fn clone(&self) -> Account {
            let _: ::core::clone::AssertParamIsClone<u64>;
            let _: ::core::clone::AssertParamIsClone<U256>;
            let _: ::core::clone::AssertParamIsClone<H256>;
            *self
        }
    }
    #[automatically_derived]
    impl ::core::marker::Copy for Account {}
    #[automatically_derived]
    impl ::core::fmt::Debug for Account {
        fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
            ::core::fmt::Formatter::debug_struct_field3_finish(
                f,
                "Account",
                "nonce",
                &&self.nonce,
                "balance",
                &&self.balance,
                "bytecode_hash",
                &&self.bytecode_hash,
            )
        }
    }
    impl ::core::marker::StructuralPartialEq for Account {}
    #[automatically_derived]
    impl ::core::cmp::PartialEq for Account {
        #[inline]
        fn eq(&self, other: &Account) -> bool {
            self.nonce == other.nonce && self.balance == other.balance
                && self.bytecode_hash == other.bytecode_hash
        }
    }
    impl ::core::marker::StructuralEq for Account {}
    #[automatically_derived]
    impl ::core::cmp::Eq for Account {
        #[inline]
        #[doc(hidden)]
        #[no_coverage]
        fn assert_receiver_is_total_eq(&self) -> () {
            let _: ::core::cmp::AssertParamIsEq<u64>;
            let _: ::core::cmp::AssertParamIsEq<U256>;
            let _: ::core::cmp::AssertParamIsEq<H256>;
        }
    }
    #[allow(deprecated)]
    const _: () = {
        #[automatically_derived]
        impl ::parity_scale_codec::Encode for Account {
            fn encode_to<
                __CodecOutputEdqy: ::parity_scale_codec::Output + ?::core::marker::Sized,
            >(&self, __codec_dest_edqy: &mut __CodecOutputEdqy) {
                {
                    ::parity_scale_codec::Encode::encode_to(
                        &<<u64 as ::parity_scale_codec::HasCompact>::Type as ::parity_scale_codec::EncodeAsRef<
                            '_,
                            u64,
                        >>::RefType::from(&self.nonce),
                        __codec_dest_edqy,
                    );
                }
                ::parity_scale_codec::Encode::encode_to(
                    &self.balance,
                    __codec_dest_edqy,
                );
                ::parity_scale_codec::Encode::encode_to(
                    &self.bytecode_hash,
                    __codec_dest_edqy,
                );
            }
        }
        #[automatically_derived]
        impl ::parity_scale_codec::EncodeLike for Account {}
    };
    #[allow(deprecated)]
    const _: () = {
        #[automatically_derived]
        impl ::parity_scale_codec::Decode for Account {
            fn decode<__CodecInputEdqy: ::parity_scale_codec::Input>(
                __codec_input_edqy: &mut __CodecInputEdqy,
            ) -> ::core::result::Result<Self, ::parity_scale_codec::Error> {
                ::core::result::Result::Ok(Account {
                    nonce: {
                        let __codec_res_edqy = <<u64 as ::parity_scale_codec::HasCompact>::Type as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Account::nonce`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy.into()
                            }
                        }
                    },
                    balance: {
                        let __codec_res_edqy = <U256 as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Account::balance`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    bytecode_hash: {
                        let __codec_res_edqy = <H256 as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Account::bytecode_hash`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                })
            }
        }
    };
}
mod block {
    use crate::{Header, HeaderLocked, Receipt, Transaction, TransactionSigned, H256};
    use std::ops::Deref;
    /// Ethereum full block.
    pub struct Block {
        /// Block header.
        pub header: Header,
        /// Transactions in this block.
        pub body: Vec<Transaction>,
        /// Block receipts.
        pub receipts: Vec<Receipt>,
    }
    #[automatically_derived]
    impl ::core::fmt::Debug for Block {
        fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
            ::core::fmt::Formatter::debug_struct_field3_finish(
                f,
                "Block",
                "header",
                &&self.header,
                "body",
                &&self.body,
                "receipts",
                &&self.receipts,
            )
        }
    }
    #[automatically_derived]
    impl ::core::clone::Clone for Block {
        #[inline]
        fn clone(&self) -> Block {
            Block {
                header: ::core::clone::Clone::clone(&self.header),
                body: ::core::clone::Clone::clone(&self.body),
                receipts: ::core::clone::Clone::clone(&self.receipts),
            }
        }
    }
    impl ::core::marker::StructuralPartialEq for Block {}
    #[automatically_derived]
    impl ::core::cmp::PartialEq for Block {
        #[inline]
        fn eq(&self, other: &Block) -> bool {
            self.header == other.header && self.body == other.body
                && self.receipts == other.receipts
        }
    }
    impl ::core::marker::StructuralEq for Block {}
    #[automatically_derived]
    impl ::core::cmp::Eq for Block {
        #[inline]
        #[doc(hidden)]
        #[no_coverage]
        fn assert_receiver_is_total_eq(&self) -> () {
            let _: ::core::cmp::AssertParamIsEq<Header>;
            let _: ::core::cmp::AssertParamIsEq<Vec<Transaction>>;
        }
    }
    #[automatically_derived]
    impl ::core::default::Default for Block {
        #[inline]
        fn default() -> Block {
            Block {
                header: ::core::default::Default::default(),
                body: ::core::default::Default::default(),
                receipts: ::core::default::Default::default(),
            }
        }
    }
    impl Deref for Block {
        type Target = Header;
        fn deref(&self) -> &Self::Target {
            &self.header
        }
    }
    /// Sealed Ethereum full block.
    pub struct BlockLocked {
        /// Locked block header.
        pub header: HeaderLocked,
        /// Transactions with signatures.
        pub body: Vec<TransactionSigned>,
        /// Block receipts.
        pub receipts: Vec<Receipt>,
    }
    #[automatically_derived]
    impl ::core::fmt::Debug for BlockLocked {
        fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
            ::core::fmt::Formatter::debug_struct_field3_finish(
                f,
                "BlockLocked",
                "header",
                &&self.header,
                "body",
                &&self.body,
                "receipts",
                &&self.receipts,
            )
        }
    }
    #[automatically_derived]
    impl ::core::clone::Clone for BlockLocked {
        #[inline]
        fn clone(&self) -> BlockLocked {
            BlockLocked {
                header: ::core::clone::Clone::clone(&self.header),
                body: ::core::clone::Clone::clone(&self.body),
                receipts: ::core::clone::Clone::clone(&self.receipts),
            }
        }
    }
    impl ::core::marker::StructuralPartialEq for BlockLocked {}
    #[automatically_derived]
    impl ::core::cmp::PartialEq for BlockLocked {
        #[inline]
        fn eq(&self, other: &BlockLocked) -> bool {
            self.header == other.header && self.body == other.body
                && self.receipts == other.receipts
        }
    }
    impl ::core::marker::StructuralEq for BlockLocked {}
    #[automatically_derived]
    impl ::core::cmp::Eq for BlockLocked {
        #[inline]
        #[doc(hidden)]
        #[no_coverage]
        fn assert_receiver_is_total_eq(&self) -> () {
            let _: ::core::cmp::AssertParamIsEq<HeaderLocked>;
            let _: ::core::cmp::AssertParamIsEq<Vec<TransactionSigned>>;
        }
    }
    #[automatically_derived]
    impl ::core::default::Default for BlockLocked {
        #[inline]
        fn default() -> BlockLocked {
            BlockLocked {
                header: ::core::default::Default::default(),
                body: ::core::default::Default::default(),
                receipts: ::core::default::Default::default(),
            }
        }
    }
    impl BlockLocked {
        /// Header hash.
        pub fn hash(&self) -> H256 {
            self.header.hash()
        }
    }
    impl Deref for BlockLocked {
        type Target = Header;
        fn deref(&self) -> &Self::Target {
            self.header.as_ref()
        }
    }
}
mod chain {
    use crate::U256;
    use ethers_core::types::{ParseChainError, U64};
    use fastrlp::{Decodable, Encodable};
    use std::{fmt, str::FromStr};
    /// Either a named or chain id or the actual id value
    pub enum Chain {
        /// Contains a known chain
        Named(ethers_core::types::Chain),
        /// Contains the id of a chain
        Id(u64),
    }
    #[automatically_derived]
    impl ::core::fmt::Debug for Chain {
        fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
            match self {
                Chain::Named(__self_0) => {
                    ::core::fmt::Formatter::debug_tuple_field1_finish(
                        f,
                        "Named",
                        &__self_0,
                    )
                }
                Chain::Id(__self_0) => {
                    ::core::fmt::Formatter::debug_tuple_field1_finish(f, "Id", &__self_0)
                }
            }
        }
    }
    #[automatically_derived]
    impl ::core::clone::Clone for Chain {
        #[inline]
        fn clone(&self) -> Chain {
            let _: ::core::clone::AssertParamIsClone<ethers_core::types::Chain>;
            let _: ::core::clone::AssertParamIsClone<u64>;
            *self
        }
    }
    #[automatically_derived]
    impl ::core::marker::Copy for Chain {}
    impl ::core::marker::StructuralPartialEq for Chain {}
    #[automatically_derived]
    impl ::core::cmp::PartialEq for Chain {
        #[inline]
        fn eq(&self, other: &Chain) -> bool {
            let __self_tag = ::core::intrinsics::discriminant_value(self);
            let __arg1_tag = ::core::intrinsics::discriminant_value(other);
            __self_tag == __arg1_tag
                && match (self, other) {
                    (Chain::Named(__self_0), Chain::Named(__arg1_0)) => {
                        *__self_0 == *__arg1_0
                    }
                    (Chain::Id(__self_0), Chain::Id(__arg1_0)) => *__self_0 == *__arg1_0,
                    _ => unsafe { ::core::intrinsics::unreachable() }
                }
        }
    }
    impl ::core::marker::StructuralEq for Chain {}
    #[automatically_derived]
    impl ::core::cmp::Eq for Chain {
        #[inline]
        #[doc(hidden)]
        #[no_coverage]
        fn assert_receiver_is_total_eq(&self) -> () {
            let _: ::core::cmp::AssertParamIsEq<ethers_core::types::Chain>;
            let _: ::core::cmp::AssertParamIsEq<u64>;
        }
    }
    #[automatically_derived]
    impl ::core::hash::Hash for Chain {
        fn hash<__H: ::core::hash::Hasher>(&self, state: &mut __H) -> () {
            let __self_tag = ::core::intrinsics::discriminant_value(self);
            ::core::hash::Hash::hash(&__self_tag, state);
            match self {
                Chain::Named(__self_0) => ::core::hash::Hash::hash(__self_0, state),
                Chain::Id(__self_0) => ::core::hash::Hash::hash(__self_0, state),
            }
        }
    }
    impl Chain {
        /// The id of the chain
        pub fn id(&self) -> u64 {
            match self {
                Chain::Named(chain) => *chain as u64,
                Chain::Id(id) => *id,
            }
        }
        /// Helper function for checking if a chainid corresponds to a legacy chainid
        /// without eip1559
        pub fn is_legacy(&self) -> bool {
            match self {
                Chain::Named(c) => c.is_legacy(),
                Chain::Id(_) => false,
            }
        }
    }
    impl fmt::Display for Chain {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Chain::Named(chain) => chain.fmt(f),
                Chain::Id(id) => {
                    if let Ok(chain) = ethers_core::types::Chain::try_from(*id) {
                        chain.fmt(f)
                    } else {
                        id.fmt(f)
                    }
                }
            }
        }
    }
    impl From<ethers_core::types::Chain> for Chain {
        fn from(id: ethers_core::types::Chain) -> Self {
            Chain::Named(id)
        }
    }
    impl From<u64> for Chain {
        fn from(id: u64) -> Self {
            ethers_core::types::Chain::try_from(id)
                .map(Chain::Named)
                .unwrap_or_else(|_| Chain::Id(id))
        }
    }
    impl From<U256> for Chain {
        fn from(id: U256) -> Self {
            id.as_u64().into()
        }
    }
    impl From<Chain> for u64 {
        fn from(c: Chain) -> Self {
            match c {
                Chain::Named(c) => c as u64,
                Chain::Id(id) => id,
            }
        }
    }
    impl From<Chain> for U64 {
        fn from(c: Chain) -> Self {
            u64::from(c).into()
        }
    }
    impl From<Chain> for U256 {
        fn from(c: Chain) -> Self {
            u64::from(c).into()
        }
    }
    impl TryFrom<Chain> for ethers_core::types::Chain {
        type Error = ParseChainError;
        fn try_from(chain: Chain) -> Result<Self, Self::Error> {
            match chain {
                Chain::Named(chain) => Ok(chain),
                Chain::Id(id) => id.try_into(),
            }
        }
    }
    impl FromStr for Chain {
        type Err = String;
        fn from_str(s: &str) -> Result<Self, Self::Err> {
            if let Ok(chain) = ethers_core::types::Chain::from_str(s) {
                Ok(Chain::Named(chain))
            } else {
                s.parse::<u64>()
                    .map(Chain::Id)
                    .map_err(|_| {
                        let res = ::alloc::fmt::format(
                            ::core::fmt::Arguments::new_v1(
                                &["Expected known chain or integer, found: "],
                                &[::core::fmt::ArgumentV1::new_display(&s)],
                            ),
                        );
                        res
                    })
            }
        }
    }
    impl Encodable for Chain {
        fn length(&self) -> usize {
            match self {
                Self::Named(chain) => u64::from(*chain).length(),
                Self::Id(id) => id.length(),
            }
        }
        fn encode(&self, out: &mut dyn fastrlp::BufMut) {
            match self {
                Self::Named(chain) => u64::from(*chain).encode(out),
                Self::Id(id) => id.encode(out),
            }
        }
    }
    impl Decodable for Chain {
        fn decode(buf: &mut &[u8]) -> Result<Self, fastrlp::DecodeError> {
            Ok(u64::decode(buf)?.into())
        }
    }
    impl Default for Chain {
        fn default() -> Self {
            ethers_core::types::Chain::Mainnet.into()
        }
    }
}
mod header {
    use std::ops::Deref;
    use crate::{BlockNumber, H160, H256, U256};
    use codecs::*;
    /// Block header
    pub struct Header {
        /// The Keccak 256-bit hash of the parent
        /// block’s header, in its entirety; formally Hp.
        pub parent_hash: H256,
        /// The Keccak 256-bit hash of the ommers list portion of this block; formally Ho.
        pub ommers_hash: H256,
        /// The 160-bit address to which all fees collected from the successful mining of this block
        /// be transferred; formally Hc.
        pub beneficiary: H160,
        /// The Keccak 256-bit hash of the root node of the state trie, after all transactions are
        /// executed and finalisations applied; formally Hr.
        pub state_root: H256,
        /// The Keccak 256-bit hash of the root node of the trie structure populated with each
        /// transaction in the transactions list portion of the
        /// block; formally Ht.
        pub transactions_root: H256,
        /// The Keccak 256-bit hash of the root
        /// node of the trie structure populated with the receipts of each transaction in the
        /// transactions list portion of the block; formally He.
        pub receipts_root: H256,
        /// The Bloom filter composed from indexable information (logger address and log topics)
        /// contained in each log entry from the receipt of each transaction in the transactions list;
        /// formally Hb.
        pub logs_bloom: H256,
        /// A scalar value corresponding to the difficulty level of this block. This can be calculated
        /// from the previous block’s difficulty level and the timestamp; formally Hd.
        pub difficulty: U256,
        /// A scalar value equal to the number of ancestor blocks. The genesis block has a number of
        /// zero; formally Hi.
        pub number: BlockNumber,
        /// A scalar value equal to the current limit of gas expenditure per block; formally Hl.
        #[codec(compact)]
        pub gas_limit: u64,
        /// A scalar value equal to the total gas used in transactions in this block; formally Hg.
        #[codec(compact)]
        pub gas_used: u64,
        /// A scalar value equal to the reasonable output of Unix’s time() at this block’s inception;
        /// formally Hs.
        #[codec(compact)]
        pub timestamp: u64,
        /// An arbitrary byte array containing data relevant to this block. This must be 32 bytes or
        /// fewer; formally Hx.
        pub extra_data: bytes::Bytes,
        /// A 256-bit hash which, combined with the
        /// nonce, proves that a sufficient amount of computation has been carried out on this block;
        /// formally Hm.
        pub mix_hash: H256,
        /// A 64-bit value which, combined with the mixhash, proves that a sufficient amount of
        /// computation has been carried out on this block; formally Hn.
        #[codec(compact)]
        pub nonce: u64,
        /// A scalar representing EIP1559 base fee which can move up or down each block according
        /// to a formula which is a function of gas used in parent block and gas target
        /// (block gas limit divided by elasticity multiplier) of parent block.
        /// The algorithm results in the base fee per gas increasing when blocks are
        /// above the gas target, and decreasing when blocks are below the gas target. The base fee per
        /// gas is burned.
        pub base_fee_per_gas: Option<u64>,
    }
    #[automatically_derived]
    impl ::core::fmt::Debug for Header {
        fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
            let names: &'static _ = &[
                "parent_hash",
                "ommers_hash",
                "beneficiary",
                "state_root",
                "transactions_root",
                "receipts_root",
                "logs_bloom",
                "difficulty",
                "number",
                "gas_limit",
                "gas_used",
                "timestamp",
                "extra_data",
                "mix_hash",
                "nonce",
                "base_fee_per_gas",
            ];
            let values: &[&dyn ::core::fmt::Debug] = &[
                &&self.parent_hash,
                &&self.ommers_hash,
                &&self.beneficiary,
                &&self.state_root,
                &&self.transactions_root,
                &&self.receipts_root,
                &&self.logs_bloom,
                &&self.difficulty,
                &&self.number,
                &&self.gas_limit,
                &&self.gas_used,
                &&self.timestamp,
                &&self.extra_data,
                &&self.mix_hash,
                &&self.nonce,
                &&self.base_fee_per_gas,
            ];
            ::core::fmt::Formatter::debug_struct_fields_finish(
                f,
                "Header",
                names,
                values,
            )
        }
    }
    #[automatically_derived]
    impl ::core::clone::Clone for Header {
        #[inline]
        fn clone(&self) -> Header {
            Header {
                parent_hash: ::core::clone::Clone::clone(&self.parent_hash),
                ommers_hash: ::core::clone::Clone::clone(&self.ommers_hash),
                beneficiary: ::core::clone::Clone::clone(&self.beneficiary),
                state_root: ::core::clone::Clone::clone(&self.state_root),
                transactions_root: ::core::clone::Clone::clone(&self.transactions_root),
                receipts_root: ::core::clone::Clone::clone(&self.receipts_root),
                logs_bloom: ::core::clone::Clone::clone(&self.logs_bloom),
                difficulty: ::core::clone::Clone::clone(&self.difficulty),
                number: ::core::clone::Clone::clone(&self.number),
                gas_limit: ::core::clone::Clone::clone(&self.gas_limit),
                gas_used: ::core::clone::Clone::clone(&self.gas_used),
                timestamp: ::core::clone::Clone::clone(&self.timestamp),
                extra_data: ::core::clone::Clone::clone(&self.extra_data),
                mix_hash: ::core::clone::Clone::clone(&self.mix_hash),
                nonce: ::core::clone::Clone::clone(&self.nonce),
                base_fee_per_gas: ::core::clone::Clone::clone(&self.base_fee_per_gas),
            }
        }
    }
    impl ::core::marker::StructuralPartialEq for Header {}
    #[automatically_derived]
    impl ::core::cmp::PartialEq for Header {
        #[inline]
        fn eq(&self, other: &Header) -> bool {
            self.parent_hash == other.parent_hash
                && self.ommers_hash == other.ommers_hash
                && self.beneficiary == other.beneficiary
                && self.state_root == other.state_root
                && self.transactions_root == other.transactions_root
                && self.receipts_root == other.receipts_root
                && self.logs_bloom == other.logs_bloom
                && self.difficulty == other.difficulty && self.number == other.number
                && self.gas_limit == other.gas_limit && self.gas_used == other.gas_used
                && self.timestamp == other.timestamp
                && self.extra_data == other.extra_data && self.mix_hash == other.mix_hash
                && self.nonce == other.nonce
                && self.base_fee_per_gas == other.base_fee_per_gas
        }
    }
    impl ::core::marker::StructuralEq for Header {}
    #[automatically_derived]
    impl ::core::cmp::Eq for Header {
        #[inline]
        #[doc(hidden)]
        #[no_coverage]
        fn assert_receiver_is_total_eq(&self) -> () {
            let _: ::core::cmp::AssertParamIsEq<H256>;
            let _: ::core::cmp::AssertParamIsEq<H160>;
            let _: ::core::cmp::AssertParamIsEq<U256>;
            let _: ::core::cmp::AssertParamIsEq<BlockNumber>;
            let _: ::core::cmp::AssertParamIsEq<u64>;
            let _: ::core::cmp::AssertParamIsEq<bytes::Bytes>;
            let _: ::core::cmp::AssertParamIsEq<Option<u64>>;
        }
    }
    #[automatically_derived]
    impl ::core::default::Default for Header {
        #[inline]
        fn default() -> Header {
            Header {
                parent_hash: ::core::default::Default::default(),
                ommers_hash: ::core::default::Default::default(),
                beneficiary: ::core::default::Default::default(),
                state_root: ::core::default::Default::default(),
                transactions_root: ::core::default::Default::default(),
                receipts_root: ::core::default::Default::default(),
                logs_bloom: ::core::default::Default::default(),
                difficulty: ::core::default::Default::default(),
                number: ::core::default::Default::default(),
                gas_limit: ::core::default::Default::default(),
                gas_used: ::core::default::Default::default(),
                timestamp: ::core::default::Default::default(),
                extra_data: ::core::default::Default::default(),
                mix_hash: ::core::default::Default::default(),
                nonce: ::core::default::Default::default(),
                base_fee_per_gas: ::core::default::Default::default(),
            }
        }
    }
    #[allow(deprecated)]
    const _: () = {
        #[automatically_derived]
        impl ::parity_scale_codec::Encode for Header {
            fn encode_to<
                __CodecOutputEdqy: ::parity_scale_codec::Output + ?::core::marker::Sized,
            >(&self, __codec_dest_edqy: &mut __CodecOutputEdqy) {
                ::parity_scale_codec::Encode::encode_to(
                    &self.parent_hash,
                    __codec_dest_edqy,
                );
                ::parity_scale_codec::Encode::encode_to(
                    &self.ommers_hash,
                    __codec_dest_edqy,
                );
                ::parity_scale_codec::Encode::encode_to(
                    &self.beneficiary,
                    __codec_dest_edqy,
                );
                ::parity_scale_codec::Encode::encode_to(
                    &self.state_root,
                    __codec_dest_edqy,
                );
                ::parity_scale_codec::Encode::encode_to(
                    &self.transactions_root,
                    __codec_dest_edqy,
                );
                ::parity_scale_codec::Encode::encode_to(
                    &self.receipts_root,
                    __codec_dest_edqy,
                );
                ::parity_scale_codec::Encode::encode_to(
                    &self.logs_bloom,
                    __codec_dest_edqy,
                );
                ::parity_scale_codec::Encode::encode_to(
                    &self.difficulty,
                    __codec_dest_edqy,
                );
                ::parity_scale_codec::Encode::encode_to(&self.number, __codec_dest_edqy);
                {
                    ::parity_scale_codec::Encode::encode_to(
                        &<<u64 as ::parity_scale_codec::HasCompact>::Type as ::parity_scale_codec::EncodeAsRef<
                            '_,
                            u64,
                        >>::RefType::from(&self.gas_limit),
                        __codec_dest_edqy,
                    );
                }
                {
                    ::parity_scale_codec::Encode::encode_to(
                        &<<u64 as ::parity_scale_codec::HasCompact>::Type as ::parity_scale_codec::EncodeAsRef<
                            '_,
                            u64,
                        >>::RefType::from(&self.gas_used),
                        __codec_dest_edqy,
                    );
                }
                {
                    ::parity_scale_codec::Encode::encode_to(
                        &<<u64 as ::parity_scale_codec::HasCompact>::Type as ::parity_scale_codec::EncodeAsRef<
                            '_,
                            u64,
                        >>::RefType::from(&self.timestamp),
                        __codec_dest_edqy,
                    );
                }
                ::parity_scale_codec::Encode::encode_to(
                    &self.extra_data,
                    __codec_dest_edqy,
                );
                ::parity_scale_codec::Encode::encode_to(
                    &self.mix_hash,
                    __codec_dest_edqy,
                );
                {
                    ::parity_scale_codec::Encode::encode_to(
                        &<<u64 as ::parity_scale_codec::HasCompact>::Type as ::parity_scale_codec::EncodeAsRef<
                            '_,
                            u64,
                        >>::RefType::from(&self.nonce),
                        __codec_dest_edqy,
                    );
                }
                ::parity_scale_codec::Encode::encode_to(
                    &self.base_fee_per_gas,
                    __codec_dest_edqy,
                );
            }
        }
        #[automatically_derived]
        impl ::parity_scale_codec::EncodeLike for Header {}
    };
    #[allow(deprecated)]
    const _: () = {
        #[automatically_derived]
        impl ::parity_scale_codec::Decode for Header {
            fn decode<__CodecInputEdqy: ::parity_scale_codec::Input>(
                __codec_input_edqy: &mut __CodecInputEdqy,
            ) -> ::core::result::Result<Self, ::parity_scale_codec::Error> {
                ::core::result::Result::Ok(Header {
                    parent_hash: {
                        let __codec_res_edqy = <H256 as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::parent_hash`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    ommers_hash: {
                        let __codec_res_edqy = <H256 as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::ommers_hash`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    beneficiary: {
                        let __codec_res_edqy = <H160 as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::beneficiary`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    state_root: {
                        let __codec_res_edqy = <H256 as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::state_root`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    transactions_root: {
                        let __codec_res_edqy = <H256 as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::transactions_root`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    receipts_root: {
                        let __codec_res_edqy = <H256 as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::receipts_root`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    logs_bloom: {
                        let __codec_res_edqy = <H256 as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::logs_bloom`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    difficulty: {
                        let __codec_res_edqy = <U256 as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::difficulty`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    number: {
                        let __codec_res_edqy = <BlockNumber as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::number`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    gas_limit: {
                        let __codec_res_edqy = <<u64 as ::parity_scale_codec::HasCompact>::Type as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::gas_limit`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy.into()
                            }
                        }
                    },
                    gas_used: {
                        let __codec_res_edqy = <<u64 as ::parity_scale_codec::HasCompact>::Type as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::gas_used`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy.into()
                            }
                        }
                    },
                    timestamp: {
                        let __codec_res_edqy = <<u64 as ::parity_scale_codec::HasCompact>::Type as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::timestamp`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy.into()
                            }
                        }
                    },
                    extra_data: {
                        let __codec_res_edqy = <bytes::Bytes as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::extra_data`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    mix_hash: {
                        let __codec_res_edqy = <H256 as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::mix_hash`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    nonce: {
                        let __codec_res_edqy = <<u64 as ::parity_scale_codec::HasCompact>::Type as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::nonce`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy.into()
                            }
                        }
                    },
                    base_fee_per_gas: {
                        let __codec_res_edqy = <Option<
                            u64,
                        > as ::parity_scale_codec::Decode>::decode(__codec_input_edqy);
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Header::base_fee_per_gas`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                })
            }
        }
    };
    impl Header {
        /// Heavy function that will calculate hash of data and will *not* save the change to metadata.
        /// Use lock, HeaderLocked and unlock if you need hash to be persistent.
        pub fn hash_slow(&self) -> H256 {
            ::core::panicking::panic("not yet implemented")
        }
        /// Calculate hash and lock the Header so that it can't be changed.
        pub fn lock(self) -> HeaderLocked {
            let hash = self.hash_slow();
            HeaderLocked { header: self, hash }
        }
    }
    /// HeaderLocked that has precalculated hash, use unlock if you want to modify header.
    pub struct HeaderLocked {
        /// Locked Header fields.
        header: Header,
        /// Locked Header hash.
        hash: H256,
    }
    #[automatically_derived]
    impl ::core::fmt::Debug for HeaderLocked {
        fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
            ::core::fmt::Formatter::debug_struct_field2_finish(
                f,
                "HeaderLocked",
                "header",
                &&self.header,
                "hash",
                &&self.hash,
            )
        }
    }
    #[automatically_derived]
    impl ::core::clone::Clone for HeaderLocked {
        #[inline]
        fn clone(&self) -> HeaderLocked {
            HeaderLocked {
                header: ::core::clone::Clone::clone(&self.header),
                hash: ::core::clone::Clone::clone(&self.hash),
            }
        }
    }
    impl ::core::marker::StructuralPartialEq for HeaderLocked {}
    #[automatically_derived]
    impl ::core::cmp::PartialEq for HeaderLocked {
        #[inline]
        fn eq(&self, other: &HeaderLocked) -> bool {
            self.header == other.header && self.hash == other.hash
        }
    }
    impl ::core::marker::StructuralEq for HeaderLocked {}
    #[automatically_derived]
    impl ::core::cmp::Eq for HeaderLocked {
        #[inline]
        #[doc(hidden)]
        #[no_coverage]
        fn assert_receiver_is_total_eq(&self) -> () {
            let _: ::core::cmp::AssertParamIsEq<Header>;
            let _: ::core::cmp::AssertParamIsEq<H256>;
        }
    }
    #[automatically_derived]
    impl ::core::default::Default for HeaderLocked {
        #[inline]
        fn default() -> HeaderLocked {
            HeaderLocked {
                header: ::core::default::Default::default(),
                hash: ::core::default::Default::default(),
            }
        }
    }
    impl AsRef<Header> for HeaderLocked {
        fn as_ref(&self) -> &Header {
            &self.header
        }
    }
    impl Deref for HeaderLocked {
        type Target = Header;
        fn deref(&self) -> &Self::Target {
            &self.header
        }
    }
    impl HeaderLocked {
        /// Extract raw header that can be modified.
        pub fn unlock(self) -> Header {
            self.header
        }
        /// Return header/block hash.
        pub fn hash(&self) -> H256 {
            self.hash
        }
    }
}
mod jsonu256 {
    use crate::U256;
    use serde::{
        de::{Error, Visitor},
        Deserialize, Deserializer, Serialize, Serializer,
    };
    use std::{fmt, str::FromStr};
    /// Wrapper around primitive U256 type to handle edge cases of json parser
    pub struct JsonU256(pub U256);
    #[automatically_derived]
    impl ::core::default::Default for JsonU256 {
        #[inline]
        fn default() -> JsonU256 {
            JsonU256(::core::default::Default::default())
        }
    }
    #[automatically_derived]
    impl ::core::fmt::Debug for JsonU256 {
        fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
            ::core::fmt::Formatter::debug_tuple_field1_finish(f, "JsonU256", &&self.0)
        }
    }
    impl ::core::marker::StructuralPartialEq for JsonU256 {}
    #[automatically_derived]
    impl ::core::cmp::PartialEq for JsonU256 {
        #[inline]
        fn eq(&self, other: &JsonU256) -> bool {
            self.0 == other.0
        }
    }
    impl ::core::marker::StructuralEq for JsonU256 {}
    #[automatically_derived]
    impl ::core::cmp::Eq for JsonU256 {
        #[inline]
        #[doc(hidden)]
        #[no_coverage]
        fn assert_receiver_is_total_eq(&self) -> () {
            let _: ::core::cmp::AssertParamIsEq<U256>;
        }
    }
    #[automatically_derived]
    impl ::core::cmp::PartialOrd for JsonU256 {
        #[inline]
        fn partial_cmp(
            &self,
            other: &JsonU256,
        ) -> ::core::option::Option<::core::cmp::Ordering> {
            ::core::cmp::PartialOrd::partial_cmp(&self.0, &other.0)
        }
    }
    #[automatically_derived]
    impl ::core::cmp::Ord for JsonU256 {
        #[inline]
        fn cmp(&self, other: &JsonU256) -> ::core::cmp::Ordering {
            ::core::cmp::Ord::cmp(&self.0, &other.0)
        }
    }
    #[automatically_derived]
    impl ::core::hash::Hash for JsonU256 {
        fn hash<__H: ::core::hash::Hasher>(&self, state: &mut __H) -> () {
            ::core::hash::Hash::hash(&self.0, state)
        }
    }
    #[automatically_derived]
    impl ::core::clone::Clone for JsonU256 {
        #[inline]
        fn clone(&self) -> JsonU256 {
            let _: ::core::clone::AssertParamIsClone<U256>;
            *self
        }
    }
    #[automatically_derived]
    impl ::core::marker::Copy for JsonU256 {}
    impl Serialize for JsonU256 {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            self.0.to_string().serialize(serializer)
        }
    }
    impl<'a> Deserialize<'a> for JsonU256 {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'a>,
        {
            deserializer.deserialize_any(JsonU256Visitor)
        }
    }
    struct JsonU256Visitor;
    impl<'a> Visitor<'a> for JsonU256Visitor {
        type Value = JsonU256;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .write_fmt(
                    ::core::fmt::Arguments::new_v1(
                        &["a hex encoding or decimal number"],
                        &[],
                    ),
                )
        }
        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: Error,
        {
            Ok(JsonU256(U256::from(value)))
        }
        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: Error,
        {
            let value = match value.len() {
                0 => U256::from(0),
                2 if value.starts_with("0x") => U256::zero(),
                _ if value.starts_with("0x") => {
                    U256::from_str(&value[2..])
                        .map_err(|e| {
                            Error::custom({
                                let res = ::alloc::fmt::format(
                                    ::core::fmt::Arguments::new_v1(
                                        &["Parsin JsonU256 as hex failed ", ": "],
                                        &[
                                            ::core::fmt::ArgumentV1::new_display(&value),
                                            ::core::fmt::ArgumentV1::new_display(&e),
                                        ],
                                    ),
                                );
                                res
                            })
                        })?
                }
                _ => {
                    U256::from_dec_str(value)
                        .map_err(|e| {
                            Error::custom({
                                let res = ::alloc::fmt::format(
                                    ::core::fmt::Arguments::new_v1(
                                        &["Parsin JsonU256 as decimal failed ", ": "],
                                        &[
                                            ::core::fmt::ArgumentV1::new_display(&value),
                                            ::core::fmt::ArgumentV1::new_debug(&e),
                                        ],
                                    ),
                                );
                                res
                            })
                        })?
                }
            };
            Ok(JsonU256(value))
        }
        fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
        where
            E: Error,
        {
            self.visit_str(value.as_ref())
        }
    }
}
mod log {
    use crate::{Address, H256};
    use codecs::main_codec;
    /// Ethereum Log
    pub struct Log {
        /// Contract that emitted this log.
        pub address: Address,
        /// Topics of the log. The number of logs depend on what `LOG` opcode is used.
        pub topics: Vec<H256>,
        /// Arbitrary length data.
        pub data: bytes::Bytes,
    }
    #[automatically_derived]
    impl ::core::clone::Clone for Log {
        #[inline]
        fn clone(&self) -> Log {
            Log {
                address: ::core::clone::Clone::clone(&self.address),
                topics: ::core::clone::Clone::clone(&self.topics),
                data: ::core::clone::Clone::clone(&self.data),
            }
        }
    }
    #[automatically_derived]
    impl ::core::fmt::Debug for Log {
        fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
            ::core::fmt::Formatter::debug_struct_field3_finish(
                f,
                "Log",
                "address",
                &&self.address,
                "topics",
                &&self.topics,
                "data",
                &&self.data,
            )
        }
    }
    impl ::core::marker::StructuralPartialEq for Log {}
    #[automatically_derived]
    impl ::core::cmp::PartialEq for Log {
        #[inline]
        fn eq(&self, other: &Log) -> bool {
            self.address == other.address && self.topics == other.topics
                && self.data == other.data
        }
    }
    impl ::core::marker::StructuralEq for Log {}
    #[automatically_derived]
    impl ::core::cmp::Eq for Log {
        #[inline]
        #[doc(hidden)]
        #[no_coverage]
        fn assert_receiver_is_total_eq(&self) -> () {
            let _: ::core::cmp::AssertParamIsEq<Address>;
            let _: ::core::cmp::AssertParamIsEq<Vec<H256>>;
            let _: ::core::cmp::AssertParamIsEq<bytes::Bytes>;
        }
    }
    #[allow(deprecated)]
    const _: () = {
        #[automatically_derived]
        impl ::parity_scale_codec::Encode for Log {
            fn encode_to<
                __CodecOutputEdqy: ::parity_scale_codec::Output + ?::core::marker::Sized,
            >(&self, __codec_dest_edqy: &mut __CodecOutputEdqy) {
                ::parity_scale_codec::Encode::encode_to(
                    &self.address,
                    __codec_dest_edqy,
                );
                ::parity_scale_codec::Encode::encode_to(&self.topics, __codec_dest_edqy);
                ::parity_scale_codec::Encode::encode_to(&self.data, __codec_dest_edqy);
            }
        }
        #[automatically_derived]
        impl ::parity_scale_codec::EncodeLike for Log {}
    };
    #[allow(deprecated)]
    const _: () = {
        #[automatically_derived]
        impl ::parity_scale_codec::Decode for Log {
            fn decode<__CodecInputEdqy: ::parity_scale_codec::Input>(
                __codec_input_edqy: &mut __CodecInputEdqy,
            ) -> ::core::result::Result<Self, ::parity_scale_codec::Error> {
                ::core::result::Result::Ok(Log {
                    address: {
                        let __codec_res_edqy = <Address as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Log::address`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    topics: {
                        let __codec_res_edqy = <Vec<
                            H256,
                        > as ::parity_scale_codec::Decode>::decode(__codec_input_edqy);
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Log::topics`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    data: {
                        let __codec_res_edqy = <bytes::Bytes as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Log::data`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                })
            }
        }
    };
}
mod receipt {
    use crate::{Log, TxType, H256};
    use codecs::main_codec;
    /// Receipt containing result of transaction execution.
    pub struct Receipt {
        /// Receipt type.
        pub tx_type: TxType,
        /// If transaction is executed successfully.
        pub success: bool,
        /// Gas used
        #[codec(compact)]
        pub cumulative_gas_used: u64,
        /// Bloom filter.
        pub bloom: H256,
        /// Log send from contracts.
        pub logs: Vec<Log>,
    }
    #[automatically_derived]
    impl ::core::clone::Clone for Receipt {
        #[inline]
        fn clone(&self) -> Receipt {
            Receipt {
                tx_type: ::core::clone::Clone::clone(&self.tx_type),
                success: ::core::clone::Clone::clone(&self.success),
                cumulative_gas_used: ::core::clone::Clone::clone(
                    &self.cumulative_gas_used,
                ),
                bloom: ::core::clone::Clone::clone(&self.bloom),
                logs: ::core::clone::Clone::clone(&self.logs),
            }
        }
    }
    #[automatically_derived]
    impl ::core::fmt::Debug for Receipt {
        fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
            ::core::fmt::Formatter::debug_struct_field5_finish(
                f,
                "Receipt",
                "tx_type",
                &&self.tx_type,
                "success",
                &&self.success,
                "cumulative_gas_used",
                &&self.cumulative_gas_used,
                "bloom",
                &&self.bloom,
                "logs",
                &&self.logs,
            )
        }
    }
    impl ::core::marker::StructuralPartialEq for Receipt {}
    #[automatically_derived]
    impl ::core::cmp::PartialEq for Receipt {
        #[inline]
        fn eq(&self, other: &Receipt) -> bool {
            self.tx_type == other.tx_type && self.success == other.success
                && self.cumulative_gas_used == other.cumulative_gas_used
                && self.bloom == other.bloom && self.logs == other.logs
        }
    }
    impl ::core::marker::StructuralEq for Receipt {}
    #[automatically_derived]
    impl ::core::cmp::Eq for Receipt {
        #[inline]
        #[doc(hidden)]
        #[no_coverage]
        fn assert_receiver_is_total_eq(&self) -> () {
            let _: ::core::cmp::AssertParamIsEq<TxType>;
            let _: ::core::cmp::AssertParamIsEq<bool>;
            let _: ::core::cmp::AssertParamIsEq<u64>;
            let _: ::core::cmp::AssertParamIsEq<H256>;
            let _: ::core::cmp::AssertParamIsEq<Vec<Log>>;
        }
    }
    #[allow(deprecated)]
    const _: () = {
        #[automatically_derived]
        impl ::parity_scale_codec::Encode for Receipt {
            fn encode_to<
                __CodecOutputEdqy: ::parity_scale_codec::Output + ?::core::marker::Sized,
            >(&self, __codec_dest_edqy: &mut __CodecOutputEdqy) {
                ::parity_scale_codec::Encode::encode_to(
                    &self.tx_type,
                    __codec_dest_edqy,
                );
                ::parity_scale_codec::Encode::encode_to(
                    &self.success,
                    __codec_dest_edqy,
                );
                {
                    ::parity_scale_codec::Encode::encode_to(
                        &<<u64 as ::parity_scale_codec::HasCompact>::Type as ::parity_scale_codec::EncodeAsRef<
                            '_,
                            u64,
                        >>::RefType::from(&self.cumulative_gas_used),
                        __codec_dest_edqy,
                    );
                }
                ::parity_scale_codec::Encode::encode_to(&self.bloom, __codec_dest_edqy);
                ::parity_scale_codec::Encode::encode_to(&self.logs, __codec_dest_edqy);
            }
        }
        #[automatically_derived]
        impl ::parity_scale_codec::EncodeLike for Receipt {}
    };
    #[allow(deprecated)]
    const _: () = {
        #[automatically_derived]
        impl ::parity_scale_codec::Decode for Receipt {
            fn decode<__CodecInputEdqy: ::parity_scale_codec::Input>(
                __codec_input_edqy: &mut __CodecInputEdqy,
            ) -> ::core::result::Result<Self, ::parity_scale_codec::Error> {
                ::core::result::Result::Ok(Receipt {
                    tx_type: {
                        let __codec_res_edqy = <TxType as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Receipt::tx_type`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    success: {
                        let __codec_res_edqy = <bool as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Receipt::success`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    cumulative_gas_used: {
                        let __codec_res_edqy = <<u64 as ::parity_scale_codec::HasCompact>::Type as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Receipt::cumulative_gas_used`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy.into()
                            }
                        }
                    },
                    bloom: {
                        let __codec_res_edqy = <H256 as ::parity_scale_codec::Decode>::decode(
                            __codec_input_edqy,
                        );
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Receipt::bloom`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                    logs: {
                        let __codec_res_edqy = <Vec<
                            Log,
                        > as ::parity_scale_codec::Decode>::decode(__codec_input_edqy);
                        match __codec_res_edqy {
                            ::core::result::Result::Err(e) => {
                                return ::core::result::Result::Err(
                                    e.chain("Could not decode `Receipt::logs`"),
                                );
                            }
                            ::core::result::Result::Ok(__codec_res_edqy) => {
                                __codec_res_edqy
                            }
                        }
                    },
                })
            }
        }
    };
}
mod transaction {
    mod access_list {
        use crate::{Address, H256};
        /// A list of addresses and storage keys that the transaction plans to access.
        /// Accesses outside the list are possible, but become more expensive.
        pub struct AccessListItem {
            /// Account addresses that would be loaded at the start of execution
            pub address: Address,
            /// Keys of storage that would be loaded at the start of execution
            pub storage_keys: Vec<H256>,
        }
        #[automatically_derived]
        impl ::core::clone::Clone for AccessListItem {
            #[inline]
            fn clone(&self) -> AccessListItem {
                AccessListItem {
                    address: ::core::clone::Clone::clone(&self.address),
                    storage_keys: ::core::clone::Clone::clone(&self.storage_keys),
                }
            }
        }
        #[automatically_derived]
        impl ::core::fmt::Debug for AccessListItem {
            fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                ::core::fmt::Formatter::debug_struct_field2_finish(
                    f,
                    "AccessListItem",
                    "address",
                    &&self.address,
                    "storage_keys",
                    &&self.storage_keys,
                )
            }
        }
        impl ::core::marker::StructuralPartialEq for AccessListItem {}
        #[automatically_derived]
        impl ::core::cmp::PartialEq for AccessListItem {
            #[inline]
            fn eq(&self, other: &AccessListItem) -> bool {
                self.address == other.address && self.storage_keys == other.storage_keys
            }
        }
        impl ::core::marker::StructuralEq for AccessListItem {}
        #[automatically_derived]
        impl ::core::cmp::Eq for AccessListItem {
            #[inline]
            #[doc(hidden)]
            #[no_coverage]
            fn assert_receiver_is_total_eq(&self) -> () {
                let _: ::core::cmp::AssertParamIsEq<Address>;
                let _: ::core::cmp::AssertParamIsEq<Vec<H256>>;
            }
        }
        /// AccessList as defined in EIP-2930
        pub type AccessList = Vec<AccessListItem>;
    }
    mod signature {
        use crate::U256;
        /// Signature TODO
        /// r, s: Values corresponding to the signature of the
        /// transaction and used to determine the sender of
        /// the transaction; formally Tr and Ts. This is expanded in Appendix F of yellow paper.
        pub struct Signature {
            /// The R field of the signature; the point on the curve.
            pub r: U256,
            /// The S field of the signature; the point on the curve.
            pub s: U256,
            /// yParity: Signature Y parity; forma Ty
            pub y_parity: u8,
        }
        #[automatically_derived]
        impl ::core::fmt::Debug for Signature {
            fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                ::core::fmt::Formatter::debug_struct_field3_finish(
                    f,
                    "Signature",
                    "r",
                    &&self.r,
                    "s",
                    &&self.s,
                    "y_parity",
                    &&self.y_parity,
                )
            }
        }
        #[automatically_derived]
        impl ::core::clone::Clone for Signature {
            #[inline]
            fn clone(&self) -> Signature {
                Signature {
                    r: ::core::clone::Clone::clone(&self.r),
                    s: ::core::clone::Clone::clone(&self.s),
                    y_parity: ::core::clone::Clone::clone(&self.y_parity),
                }
            }
        }
        impl ::core::marker::StructuralPartialEq for Signature {}
        #[automatically_derived]
        impl ::core::cmp::PartialEq for Signature {
            #[inline]
            fn eq(&self, other: &Signature) -> bool {
                self.r == other.r && self.s == other.s && self.y_parity == other.y_parity
            }
        }
        impl ::core::marker::StructuralEq for Signature {}
        #[automatically_derived]
        impl ::core::cmp::Eq for Signature {
            #[inline]
            #[doc(hidden)]
            #[no_coverage]
            fn assert_receiver_is_total_eq(&self) -> () {
                let _: ::core::cmp::AssertParamIsEq<U256>;
                let _: ::core::cmp::AssertParamIsEq<u8>;
            }
        }
    }
    mod tx_type {
        use codecs::main_codec;
        /// Transaction Type
        pub enum TxType {
            /// Legacy transaction pre EIP-2929
            Legacy = 0_u8,
            /// AccessList transaction
            EIP2930 = 1,
            /// Transaction with Priority fee
            EIP1559 = 2,
        }
        #[automatically_derived]
        impl ::core::clone::Clone for TxType {
            #[inline]
            fn clone(&self) -> TxType {
                *self
            }
        }
        #[automatically_derived]
        impl ::core::marker::Copy for TxType {}
        #[automatically_derived]
        impl ::core::fmt::Debug for TxType {
            fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                match self {
                    TxType::Legacy => ::core::fmt::Formatter::write_str(f, "Legacy"),
                    TxType::EIP2930 => ::core::fmt::Formatter::write_str(f, "EIP2930"),
                    TxType::EIP1559 => ::core::fmt::Formatter::write_str(f, "EIP1559"),
                }
            }
        }
        impl ::core::marker::StructuralPartialEq for TxType {}
        #[automatically_derived]
        impl ::core::cmp::PartialEq for TxType {
            #[inline]
            fn eq(&self, other: &TxType) -> bool {
                let __self_tag = ::core::intrinsics::discriminant_value(self);
                let __arg1_tag = ::core::intrinsics::discriminant_value(other);
                __self_tag == __arg1_tag
            }
        }
        impl ::core::marker::StructuralEq for TxType {}
        #[automatically_derived]
        impl ::core::cmp::Eq for TxType {
            #[inline]
            #[doc(hidden)]
            #[no_coverage]
            fn assert_receiver_is_total_eq(&self) -> () {}
        }
        #[automatically_derived]
        impl ::core::cmp::PartialOrd for TxType {
            #[inline]
            fn partial_cmp(
                &self,
                other: &TxType,
            ) -> ::core::option::Option<::core::cmp::Ordering> {
                let __self_tag = ::core::intrinsics::discriminant_value(self);
                let __arg1_tag = ::core::intrinsics::discriminant_value(other);
                ::core::cmp::PartialOrd::partial_cmp(&__self_tag, &__arg1_tag)
            }
        }
        #[automatically_derived]
        impl ::core::cmp::Ord for TxType {
            #[inline]
            fn cmp(&self, other: &TxType) -> ::core::cmp::Ordering {
                let __self_tag = ::core::intrinsics::discriminant_value(self);
                let __arg1_tag = ::core::intrinsics::discriminant_value(other);
                ::core::cmp::Ord::cmp(&__self_tag, &__arg1_tag)
            }
        }
    }
    use crate::{Address, Bytes, TxHash, U256};
    pub use access_list::{AccessList, AccessListItem};
    use signature::Signature;
    use std::ops::Deref;
    pub use tx_type::TxType;
    /// Raw Transaction.
    /// Transaction type is introduced in EIP-2718: https://eips.ethereum.org/EIPS/eip-2718
    pub enum Transaction {
        /// Legacy transaciton.
        Legacy {
            /// Added as EIP-155: Simple replay attack protection
            chain_id: Option<u64>,
            /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
            nonce: u64,
            /// A scalar value equal to the number of
            /// Wei to be paid per unit of gas for all computation
            /// costs incurred as a result of the execution of this transaction; formally Tp.
            gas_price: u64,
            /// A scalar value equal to the maximum
            /// amount of gas that should be used in executing
            /// this transaction. This is paid up-front, before any
            /// computation is done and may not be increased
            /// later; formally Tg.
            gas_limit: u64,
            /// The 160-bit address of the message call’s recipient or, for a contract creation
            /// transaction, ∅, used here to denote the only member of B0 ; formally Tt.
            to: Option<Address>,
            /// A scalar value equal to the number of Wei to
            /// be transferred to the message call’s recipient or,
            /// in the case of contract creation, as an endowment
            /// to the newly created account; formally Tv.
            value: U256,
            /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
            /// Some). init: An unlimited size byte array specifying the
            /// EVM-code for the account initialisation procedure CREATE,
            /// data: An unlimited size byte array specifying the
            /// input data of the message call, formally Td.
            input: Bytes,
        },
        /// Transaction with AccessList. https://eips.ethereum.org/EIPS/eip-2930
        Eip2930 {
            /// Added as EIP-155: Simple replay attack protection
            chain_id: u64,
            /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
            nonce: u64,
            /// A scalar value equal to the number of
            /// Wei to be paid per unit of gas for all computation
            /// costs incurred as a result of the execution of this transaction; formally Tp.
            gas_price: u64,
            /// A scalar value equal to the maximum
            /// amount of gas that should be used in executing
            /// this transaction. This is paid up-front, before any
            /// computation is done and may not be increased
            /// later; formally Tg.
            gas_limit: u64,
            /// The 160-bit address of the message call’s recipient or, for a contract creation
            /// transaction, ∅, used here to denote the only member of B0 ; formally Tt.
            to: Option<Address>,
            /// A scalar value equal to the number of Wei to
            /// be transferred to the message call’s recipient or,
            /// in the case of contract creation, as an endowment
            /// to the newly created account; formally Tv.
            value: U256,
            /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
            /// Some). init: An unlimited size byte array specifying the
            /// EVM-code for the account initialisation procedure CREATE,
            /// data: An unlimited size byte array specifying the
            /// input data of the message call, formally Td.
            input: Bytes,
            /// The accessList specifies a list of addresses and storage keys;
            /// these addresses and storage keys are added into the `accessed_addresses`
            /// and `accessed_storage_keys` global sets (introduced in EIP-2929).
            /// A gas cost is charged, though at a discount relative to the cost of
            /// accessing outside the list.
            access_list: AccessList,
        },
        /// Transaction with priority fee. https://eips.ethereum.org/EIPS/eip-1559
        Eip1559 {
            /// Added as EIP-155: Simple replay attack protection
            chain_id: u64,
            /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
            nonce: u64,
            /// A scalar value equal to the maximum
            /// amount of gas that should be used in executing
            /// this transaction. This is paid up-front, before any
            /// computation is done and may not be increased
            /// later; formally Tg.
            gas_limit: u64,
            /// A scalar value equal to the maximum
            /// amount of gas that should be used in executing
            /// this transaction. This is paid up-front, before any
            /// computation is done and may not be increased
            /// later; formally Tg.
            max_fee_per_gas: u64,
            /// Max Priority fee that transaction is paying
            max_priority_fee_per_gas: u64,
            /// The 160-bit address of the message call’s recipient or, for a contract creation
            /// transaction, ∅, used here to denote the only member of B0 ; formally Tt.
            to: Option<Address>,
            /// A scalar value equal to the number of Wei to
            /// be transferred to the message call’s recipient or,
            /// in the case of contract creation, as an endowment
            /// to the newly created account; formally Tv.
            value: U256,
            /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
            /// Some). init: An unlimited size byte array specifying the
            /// EVM-code for the account initialisation procedure CREATE,
            /// data: An unlimited size byte array specifying the
            /// input data of the message call, formally Td.
            input: Bytes,
            /// The accessList specifies a list of addresses and storage keys;
            /// these addresses and storage keys are added into the `accessed_addresses`
            /// and `accessed_storage_keys` global sets (introduced in EIP-2929).
            /// A gas cost is charged, though at a discount relative to the cost of
            /// accessing outside the list.
            access_list: AccessList,
        },
    }
    #[automatically_derived]
    impl ::core::fmt::Debug for Transaction {
        fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
            match self {
                Transaction::Legacy {
                    chain_id: __self_0,
                    nonce: __self_1,
                    gas_price: __self_2,
                    gas_limit: __self_3,
                    to: __self_4,
                    value: __self_5,
                    input: __self_6,
                } => {
                    let names: &'static _ = &[
                        "chain_id",
                        "nonce",
                        "gas_price",
                        "gas_limit",
                        "to",
                        "value",
                        "input",
                    ];
                    let values: &[&dyn ::core::fmt::Debug] = &[
                        &__self_0,
                        &__self_1,
                        &__self_2,
                        &__self_3,
                        &__self_4,
                        &__self_5,
                        &__self_6,
                    ];
                    ::core::fmt::Formatter::debug_struct_fields_finish(
                        f,
                        "Legacy",
                        names,
                        values,
                    )
                }
                Transaction::Eip2930 {
                    chain_id: __self_0,
                    nonce: __self_1,
                    gas_price: __self_2,
                    gas_limit: __self_3,
                    to: __self_4,
                    value: __self_5,
                    input: __self_6,
                    access_list: __self_7,
                } => {
                    let names: &'static _ = &[
                        "chain_id",
                        "nonce",
                        "gas_price",
                        "gas_limit",
                        "to",
                        "value",
                        "input",
                        "access_list",
                    ];
                    let values: &[&dyn ::core::fmt::Debug] = &[
                        &__self_0,
                        &__self_1,
                        &__self_2,
                        &__self_3,
                        &__self_4,
                        &__self_5,
                        &__self_6,
                        &__self_7,
                    ];
                    ::core::fmt::Formatter::debug_struct_fields_finish(
                        f,
                        "Eip2930",
                        names,
                        values,
                    )
                }
                Transaction::Eip1559 {
                    chain_id: __self_0,
                    nonce: __self_1,
                    gas_limit: __self_2,
                    max_fee_per_gas: __self_3,
                    max_priority_fee_per_gas: __self_4,
                    to: __self_5,
                    value: __self_6,
                    input: __self_7,
                    access_list: __self_8,
                } => {
                    let names: &'static _ = &[
                        "chain_id",
                        "nonce",
                        "gas_limit",
                        "max_fee_per_gas",
                        "max_priority_fee_per_gas",
                        "to",
                        "value",
                        "input",
                        "access_list",
                    ];
                    let values: &[&dyn ::core::fmt::Debug] = &[
                        &__self_0,
                        &__self_1,
                        &__self_2,
                        &__self_3,
                        &__self_4,
                        &__self_5,
                        &__self_6,
                        &__self_7,
                        &__self_8,
                    ];
                    ::core::fmt::Formatter::debug_struct_fields_finish(
                        f,
                        "Eip1559",
                        names,
                        values,
                    )
                }
            }
        }
    }
    #[automatically_derived]
    impl ::core::clone::Clone for Transaction {
        #[inline]
        fn clone(&self) -> Transaction {
            match self {
                Transaction::Legacy {
                    chain_id: __self_0,
                    nonce: __self_1,
                    gas_price: __self_2,
                    gas_limit: __self_3,
                    to: __self_4,
                    value: __self_5,
                    input: __self_6,
                } => {
                    Transaction::Legacy {
                        chain_id: ::core::clone::Clone::clone(__self_0),
                        nonce: ::core::clone::Clone::clone(__self_1),
                        gas_price: ::core::clone::Clone::clone(__self_2),
                        gas_limit: ::core::clone::Clone::clone(__self_3),
                        to: ::core::clone::Clone::clone(__self_4),
                        value: ::core::clone::Clone::clone(__self_5),
                        input: ::core::clone::Clone::clone(__self_6),
                    }
                }
                Transaction::Eip2930 {
                    chain_id: __self_0,
                    nonce: __self_1,
                    gas_price: __self_2,
                    gas_limit: __self_3,
                    to: __self_4,
                    value: __self_5,
                    input: __self_6,
                    access_list: __self_7,
                } => {
                    Transaction::Eip2930 {
                        chain_id: ::core::clone::Clone::clone(__self_0),
                        nonce: ::core::clone::Clone::clone(__self_1),
                        gas_price: ::core::clone::Clone::clone(__self_2),
                        gas_limit: ::core::clone::Clone::clone(__self_3),
                        to: ::core::clone::Clone::clone(__self_4),
                        value: ::core::clone::Clone::clone(__self_5),
                        input: ::core::clone::Clone::clone(__self_6),
                        access_list: ::core::clone::Clone::clone(__self_7),
                    }
                }
                Transaction::Eip1559 {
                    chain_id: __self_0,
                    nonce: __self_1,
                    gas_limit: __self_2,
                    max_fee_per_gas: __self_3,
                    max_priority_fee_per_gas: __self_4,
                    to: __self_5,
                    value: __self_6,
                    input: __self_7,
                    access_list: __self_8,
                } => {
                    Transaction::Eip1559 {
                        chain_id: ::core::clone::Clone::clone(__self_0),
                        nonce: ::core::clone::Clone::clone(__self_1),
                        gas_limit: ::core::clone::Clone::clone(__self_2),
                        max_fee_per_gas: ::core::clone::Clone::clone(__self_3),
                        max_priority_fee_per_gas: ::core::clone::Clone::clone(__self_4),
                        to: ::core::clone::Clone::clone(__self_5),
                        value: ::core::clone::Clone::clone(__self_6),
                        input: ::core::clone::Clone::clone(__self_7),
                        access_list: ::core::clone::Clone::clone(__self_8),
                    }
                }
            }
        }
    }
    impl ::core::marker::StructuralPartialEq for Transaction {}
    #[automatically_derived]
    impl ::core::cmp::PartialEq for Transaction {
        #[inline]
        fn eq(&self, other: &Transaction) -> bool {
            let __self_tag = ::core::intrinsics::discriminant_value(self);
            let __arg1_tag = ::core::intrinsics::discriminant_value(other);
            __self_tag == __arg1_tag
                && match (self, other) {
                    (
                        Transaction::Legacy {
                            chain_id: __self_0,
                            nonce: __self_1,
                            gas_price: __self_2,
                            gas_limit: __self_3,
                            to: __self_4,
                            value: __self_5,
                            input: __self_6,
                        },
                        Transaction::Legacy {
                            chain_id: __arg1_0,
                            nonce: __arg1_1,
                            gas_price: __arg1_2,
                            gas_limit: __arg1_3,
                            to: __arg1_4,
                            value: __arg1_5,
                            input: __arg1_6,
                        },
                    ) => {
                        *__self_0 == *__arg1_0 && *__self_1 == *__arg1_1
                            && *__self_2 == *__arg1_2 && *__self_3 == *__arg1_3
                            && *__self_4 == *__arg1_4 && *__self_5 == *__arg1_5
                            && *__self_6 == *__arg1_6
                    }
                    (
                        Transaction::Eip2930 {
                            chain_id: __self_0,
                            nonce: __self_1,
                            gas_price: __self_2,
                            gas_limit: __self_3,
                            to: __self_4,
                            value: __self_5,
                            input: __self_6,
                            access_list: __self_7,
                        },
                        Transaction::Eip2930 {
                            chain_id: __arg1_0,
                            nonce: __arg1_1,
                            gas_price: __arg1_2,
                            gas_limit: __arg1_3,
                            to: __arg1_4,
                            value: __arg1_5,
                            input: __arg1_6,
                            access_list: __arg1_7,
                        },
                    ) => {
                        *__self_0 == *__arg1_0 && *__self_1 == *__arg1_1
                            && *__self_2 == *__arg1_2 && *__self_3 == *__arg1_3
                            && *__self_4 == *__arg1_4 && *__self_5 == *__arg1_5
                            && *__self_6 == *__arg1_6 && *__self_7 == *__arg1_7
                    }
                    (
                        Transaction::Eip1559 {
                            chain_id: __self_0,
                            nonce: __self_1,
                            gas_limit: __self_2,
                            max_fee_per_gas: __self_3,
                            max_priority_fee_per_gas: __self_4,
                            to: __self_5,
                            value: __self_6,
                            input: __self_7,
                            access_list: __self_8,
                        },
                        Transaction::Eip1559 {
                            chain_id: __arg1_0,
                            nonce: __arg1_1,
                            gas_limit: __arg1_2,
                            max_fee_per_gas: __arg1_3,
                            max_priority_fee_per_gas: __arg1_4,
                            to: __arg1_5,
                            value: __arg1_6,
                            input: __arg1_7,
                            access_list: __arg1_8,
                        },
                    ) => {
                        *__self_0 == *__arg1_0 && *__self_1 == *__arg1_1
                            && *__self_2 == *__arg1_2 && *__self_3 == *__arg1_3
                            && *__self_4 == *__arg1_4 && *__self_5 == *__arg1_5
                            && *__self_6 == *__arg1_6 && *__self_7 == *__arg1_7
                            && *__self_8 == *__arg1_8
                    }
                    _ => unsafe { ::core::intrinsics::unreachable() }
                }
        }
    }
    impl ::core::marker::StructuralEq for Transaction {}
    #[automatically_derived]
    impl ::core::cmp::Eq for Transaction {
        #[inline]
        #[doc(hidden)]
        #[no_coverage]
        fn assert_receiver_is_total_eq(&self) -> () {
            let _: ::core::cmp::AssertParamIsEq<Option<u64>>;
            let _: ::core::cmp::AssertParamIsEq<u64>;
            let _: ::core::cmp::AssertParamIsEq<U256>;
            let _: ::core::cmp::AssertParamIsEq<Bytes>;
            let _: ::core::cmp::AssertParamIsEq<AccessList>;
        }
    }
    impl Transaction {
        /// Heavy operation that return hash over rlp encoded transaction.
        /// It is only used for signature signing.
        pub fn signature_hash(&self) -> TxHash {
            ::core::panicking::panic("not yet implemented")
        }
    }
    /// Signed transaction.
    pub struct TransactionSigned {
        transaction: Transaction,
        hash: TxHash,
        signature: Signature,
    }
    #[automatically_derived]
    impl ::core::fmt::Debug for TransactionSigned {
        fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
            ::core::fmt::Formatter::debug_struct_field3_finish(
                f,
                "TransactionSigned",
                "transaction",
                &&self.transaction,
                "hash",
                &&self.hash,
                "signature",
                &&self.signature,
            )
        }
    }
    #[automatically_derived]
    impl ::core::clone::Clone for TransactionSigned {
        #[inline]
        fn clone(&self) -> TransactionSigned {
            TransactionSigned {
                transaction: ::core::clone::Clone::clone(&self.transaction),
                hash: ::core::clone::Clone::clone(&self.hash),
                signature: ::core::clone::Clone::clone(&self.signature),
            }
        }
    }
    impl ::core::marker::StructuralPartialEq for TransactionSigned {}
    #[automatically_derived]
    impl ::core::cmp::PartialEq for TransactionSigned {
        #[inline]
        fn eq(&self, other: &TransactionSigned) -> bool {
            self.transaction == other.transaction && self.hash == other.hash
                && self.signature == other.signature
        }
    }
    impl ::core::marker::StructuralEq for TransactionSigned {}
    #[automatically_derived]
    impl ::core::cmp::Eq for TransactionSigned {
        #[inline]
        #[doc(hidden)]
        #[no_coverage]
        fn assert_receiver_is_total_eq(&self) -> () {
            let _: ::core::cmp::AssertParamIsEq<Transaction>;
            let _: ::core::cmp::AssertParamIsEq<TxHash>;
            let _: ::core::cmp::AssertParamIsEq<Signature>;
        }
    }
    impl AsRef<Transaction> for TransactionSigned {
        fn as_ref(&self) -> &Transaction {
            &self.transaction
        }
    }
    impl Deref for TransactionSigned {
        type Target = Transaction;
        fn deref(&self) -> &Self::Target {
            &self.transaction
        }
    }
    impl TransactionSigned {
        /// Transaction signature.
        pub fn signature(&self) -> &Signature {
            &self.signature
        }
        /// Transaction hash. Used to identify transaction.
        pub fn hash(&self) -> TxHash {
            self.hash
        }
    }
}
pub use account::Account;
pub use block::{Block, BlockLocked};
pub use chain::Chain;
pub use header::{Header, HeaderLocked};
pub use jsonu256::JsonU256;
pub use log::Log;
pub use receipt::Receipt;
pub use transaction::{
    AccessList, AccessListItem, Transaction, TransactionSigned, TxType,
};
/// Block hash.
pub type BlockHash = H256;
/// Block Number is height of chain
pub type BlockNumber = u64;
/// Ethereum address
pub type Address = H160;
/// BlockId is Keccak hash of the header
pub type BlockID = H256;
/// TxHash is Kecack hash of rlp encoded signed transaction
pub type TxHash = H256;
/// Storage Key
pub type StorageKey = H256;
/// Storage value
pub type StorageValue = H256;
pub use ethers_core::{
    types as rpc, types::{Bloom, Bytes, H160, H256, H512, H64, U256, U64},
};
