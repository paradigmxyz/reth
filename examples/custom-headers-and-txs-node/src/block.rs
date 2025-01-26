// use alloy_consensus::Block;
// use reth_primitives_traits::SignedTransaction;

// use crate::CustomHeader;

// #[derive(Debug, Clone)]
// pub struct CustomBlock<T> {
//     eth_block: Block<T>,
//     custom_header: CustomHeader,
// }


// impl BlockTrait for CustomBlock {
//     type Header = CustomHeader;
//     type Body = EthBlock;

//     fn new(header: Self::Header, body: Self::Body) -> Self {
//         Self { custom_header: header, eth_block: body }
//     }

//     fn header(&self) -> &Self::Header {
//         &self.custom_header
//     }

//     fn body(&self) -> &Self::Body {
//         &self.eth_block
//     }

//     fn split(self) -> (Self::Header, Self::Body) {
//         (self.custom_header, self.eth_block)
//     }
// }