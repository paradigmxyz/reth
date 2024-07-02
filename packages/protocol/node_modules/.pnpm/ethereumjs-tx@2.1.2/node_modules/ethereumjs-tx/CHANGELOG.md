# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
(modification: no type change headlines) and this project adheres to
[Semantic Versioning](http://semver.org/spec/v2.0.0.html).

## [2.1.2] - 2019-12-19

- Added support for the `MuirGlacier` HF by updating the `ethereumjs-common` dependency
  to [v1.5.0](https://github.com/ethereumjs/ethereumjs-common/releases/tag/v1.5.0)

[2.1.2]: https://github.com/ethereumjs/ethereumjs-tx/compare/v2.1.1...v2.1.2

## [2.1.1] - 2019-08-30

- Added support for `Istanbul` reduced non-zero call data gas prices
  ([EIP-2028](https://eips.ethereum.org/EIPS/eip-2028)),
  PR [#171](https://github.com/ethereumjs/ethereumjs-tx/pull/171)

[2.1.1]: https://github.com/ethereumjs/ethereumjs-tx/compare/v2.1.0...v2.1.1

## [2.1.0] - 2019-06-28

**Using testnets and custom/private networks is now easier**

This release is focused on making this library easier to use in chains other than `mainnet`.

Using standard testnets can be as easy as passing their names to the `Transaction` constructor. For
example, `new Transaction(rawTx, {chain: 'ropsten', hardfork: 'byzantium'})` is enough to use this
library with Ropsten on Byzantium.

If you are using a custom network, you can take advantage of [ethereumjs-common](https://github.com/ethereumjs/ethereumjs-common),
which contains all the network parameters. In this version of `ethereumjs-tx` you can use its new
`Common.forCustomNetwork` to create a `Common` instance based on a standard network with some
parameters changed. You can see an example of how to do this [here](https://github.com/ethereumjs/ethereumjs-common/blob/9e624f86107cea904d8171524130d92c99bf9302/src/index.ts).

List of changes:

- Upgraded [ethereumjs-common](https://github.com/ethereumjs/ethereumjs-common) to `^1.3.0`
- Added more documentation and examples on how to create transactions for public testnets and
  custom networks.

[2.1.0]: https://github.com/ethereumjs/ethereumjs-tx/compare/v2.0.0...v2.1.0

## [2.0.0] - 2019-06-03

**TypeScript / Module Import / Node Support**

First `TypeScript` based release of the library, see
PR [#145](https://github.com/ethereumjs/ethereumjs-tx/pull/145) for details.

This comes along with some changes on the API, Node import of the exposed
classes now goes like this:

```javascript
const EthereumTx = require('ethereumjs-transaction').Transaction
const FakeEthereumTx = require('ethereumjs-transaction').FakeTransaction
```

The library now also comes with a **type declaration file** distributed along
with the package published.

Along with this release we drop official support for `Node` versions `4`,`5`
and `6`. Officially tested versions are now `Node` `8`, `10` and `11`
(see PRs [#138](https://github.com/ethereumjs/ethereumjs-tx/pull/138) and
[#146](https://github.com/ethereumjs/ethereumjs-tx/pull/146)).

**Hardfork Support / Official Test Updates**

Along with a long overdue update of the official Ethereum Transaction tests
(see PRs [#131](https://github.com/ethereumjs/ethereumjs-tx/pull/131) and
[#138](https://github.com/ethereumjs/ethereumjs-tx/pull/138) for
`FakeTransaction`) and
an introduction of setting chain and hardfork by using our shared
[ethereumjs-common](https://github.com/ethereumjs/ethereumjs-common) class
(see PR [#131](https://github.com/ethereumjs/ethereumjs-tx/pull/130)) the
transaction library now supports all HFs up to the `Petersburg` hardfork,
see [constructor option docs](https://github.com/ethereumjs/ethereumjs-tx/blob/master/docs/interfaces/transactionoptions.md) for information on instantiation and default values (current hardfork default: `petersburg`).

API Changes:

- Removal of the `data.chainId` parameter, use the `opts.chain` parameter or a custom `Common` instance

**Default EIP-155 Support**

Along with defaulting to a post-`Spurious Dragon` HF replay protection from
[EIP-155](https://eips.ethereum.org/EIPS/eip-155) is now activated by default. Transactions are subsequently also by default signed with `EIP-155` replay protection,
see PRs [#153](https://github.com/ethereumjs/ethereumjs-tx/pull/153),
[#147](https://github.com/ethereumjs/ethereumjs-tx/pull/147) and
[#143](https://github.com/ethereumjs/ethereumjs-tx/pull/143).

This comes with some changes in how different `v` values passed on instantation
or changed on runtime are handled:

- The constructor throws if the `v` value is present, indicates that `EIP-155`
  was enabled, and the chain id it indicates doesn't match the one of the
  internal `common` object
- No default `v` is set. If a transaction isn't signed, it would be an empty
  buffer
- If `v` is changed after construction its value is validated in its setter

For activating non-`EIP-155` behavior instantiate the transaction with a
pre-`Spurious Dragon` hardfork option.

[2.0.0]: https://github.com/ethereumjs/ethereumjs-tx/compare/v1.3.7...v2.0.0

## [1.3.7] - 2018-07-25

- Fix bug causing `FakeTransaction.from` to not retrieve sender address from tx signature, see PR [#118](https://github.com/ethereumjs/ethereumjs-tx/pull/118)

[1.3.7]: https://github.com/ethereumjs/ethereumjs-tx/compare/v1.3.6...v1.3.7

## [1.3.6] - 2018-07-02

- Fixes issue [#108](https://github.com/ethereumjs/ethereumjs-tx/issues/108) with the `FakeTransaction.hash()` function by reverting the introduced signature handling changes in Fake transaction hash creation from PR [#94](https://github.com/ethereumjs/ethereumjs-tx/pull/94) introduced in `v1.3.5`. The signature is now again only created and added to the hash when `from` address is set and `from` is not defaulting to the zero adress any more, see PR [#110](https://github.com/ethereumjs/ethereumjs-tx/pull/110)
- Added additional tests to cover issue described above

[1.3.6]: https://github.com/ethereumjs/ethereumjs-tx/compare/v1.3.5...v1.3.6

## [1.3.5] - 2018-06-22

- Include signature by default in `FakeTransaction.hash`, PR [#97](https://github.com/ethereumjs/ethereumjs-tx/pull/97)
- Fix `FakeTransaction` signature failure bug, PR [#94](https://github.com/ethereumjs/ethereumjs-tx/pull/94)

[1.3.5]: https://github.com/ethereumjs/ethereumjs-tx/compare/v1.3.4...v1.3.5

## [1.3.4] - 2018-03-06

- Fix a bug producing hash collisions on `FakeTransaction` for different senders, PR [#81](https://github.com/ethereumjs/ethereumjs-tx/pull/81)
- Switched from deprecated `es2015` to `env` babel preset, PR [#86](https://github.com/ethereumjs/ethereumjs-tx/pull/86)
- Dropped Node 4 support

[1.3.4]: https://github.com/ethereumjs/ethereumjs-tx/compare/v1.3.3...v1.3.4

## [1.3.3] - 2017-07-12

- Allow zeros in `v`,`r`,`s` signature values
- Dropped `browserify` transform from `package.json`
- (combined v1.3.3 and v1.3.2 release notes)

[1.3.3]: https://github.com/ethereumjs/ethereumjs-tx/compare/v1.3.1...v1.3.3

## [1.3.1] - 2017-05-13

- Added `ES5` build

[1.3.1]: https://github.com/ethereumjs/ethereumjs-tx/compare/v1.3.0...v1.3.1

## [1.3.0] - 2017-04-24

- `EIP155`: allow `v` value to be greater than one byte (replay attack protection)
- Added `browserify` `ES2015` transform to `package.json`
- Improved documentation
- (combined v1.3.0, v1.2.5 and v1.2.4 release notes)

[1.3.0]: https://github.com/ethereumjs/ethereumjs-tx/compare/v1.2.3...v1.3.0

## [1.2.3] - 2017-01-30

- `EIP155` hash implementation
- README example and doc fixes

[1.2.3]: https://github.com/ethereumjs/ethereumjs-tx/compare/v1.2.2...v1.2.3

## [1.2.2] - 2016-12-15

- Moved `chainId` param to `txParams`, parse `sig` for `chainId` (`EIP155` refactor)
- Test improvements
- (combined v1.2.2 and v1.2.1 release notes)

[1.2.2]: https://github.com/ethereumjs/ethereumjs-tx/compare/v1.2.0...v1.2.2

## [1.2.0] - 2016-12-14

- Added `EIP155` changes
- Renamed `chain_id` to `chainId`
- Node 4/5 compatibility
- `ES6` standards

[1.2.0]: https://github.com/ethereumjs/ethereumjs-tx/compare/v1.1.4...v1.2.0

## Older releases:

- [1.1.4](https://github.com/ethereumjs/ethereumjs-tx/compare/v1.1.3...v1.1.4) - 2016-11-17
- [1.1.3](https://github.com/ethereumjs/ethereumjs-tx/compare/v1.1.2...v1.1.3) - 2016-11-10
- [1.1.2](https://github.com/ethereumjs/ethereumjs-tx/compare/v1.1.1...v1.1.2) - 2016-07-17
- [1.1.1](https://github.com/ethereumjs/ethereumjs-tx/compare/v1.1.0...v1.1.1) - 2016-03-05
- [1.1.0](https://github.com/ethereumjs/ethereumjs-tx/compare/v1.0.1...v1.1.0) - 2016-03-03
- [1.0.1](https://github.com/ethereumjs/ethereumjs-tx/compare/v1.0.0...v1.0.1) - 2016-03-03
- [1.0.0](https://github.com/ethereumjs/ethereumjs-tx/compare/v0.7.3...v1.0.0) - 2016-02-11
