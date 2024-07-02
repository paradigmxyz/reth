## About the compiled contracts

Following Circle's recommendation for native token support (USDC, EURC), one needs to follow the standard proposed below:

https://github.com/circlefin/stablecoin-evm/blob/master/doc/bridged_USDC_standard.md

According to this document:

> The third-party team’s bridged USDC token contract is expected to be identical to native USDC token contracts on other EVM blockchains. USDC uses a proxy pattern, so the standard applies to both the implementation contract code and the token proxy.
>
> Using identical code facilitates trustless contract verification by Circle and supports a seamless integration with existing USDC services. To facilitate this, the third-party team may choose one of the following:
>
> Copy previously deployed bytecode from a recent, native USDC token contract deployment (both proxy and implementation) on an EVM blockchain, for example Arbitrum, Base, OP Mainnet, or Polygon PoS Note that you must supply different constructor and initializer parameters where needed.
>
> Build the FiatToken contracts from source. In this case, the compiler metadata must be published or made available to support full contract verification. Various suggested compiler settings that Circle uses can be found here, which will allow the third-party team to reach the same bytecode if followed consistently.

Following the recommendations the contracts were built with the same compiler settings (version + optimization) and they have bytecode equivalance with the other contracts (mentioned in the doc, and can be found on links below (Arbitrum, Scroll, Polygon, etc.)).

For reference, here are Arbitrum's proxy + token contracts:

- Proxy: https://arbiscan.io/token/0xaf88d065e77c8cc2239327c5edb3a432268e5831#code
- Implementation: https://arbiscan.io/address/0x0f4fb9474303d10905AB86aA8d5A65FE44b6E04A#code

As a cross-reference, one can compare the bytecode of the ones present on arbiscan and here in the .json files (under bytcode key), the additional (meta)data could be helpful for contracts verfication.
