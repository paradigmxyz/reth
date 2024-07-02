# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
(modification: no type change headlines) and this project adheres to
[Semantic Versioning](http://semver.org/spec/v2.0.0.html).

## [1.5.0] - 2019-12-10

Support for the `MuirGlacier` HF
([EIP-2387](https://eips.ethereum.org/EIPS/eip-2387)) scheduled for January 2020
delaying the difficulty bomb.

Changes:

- Implemented [EIP-2384](https://eips.ethereum.org/EIPS/eip-2384) Difficulty
  Bomb Delay, PR [#75](https://github.com/ethereumjs/ethereumjs-common/pull/75)
- Consistent genesis account balance format, converted from decimal to hex
  where necessary, PR [#73](https://github.com/ethereumjs/ethereumjs-common/pull/73)

[1.5.0]: https://github.com/ethereumjs/ethereumjs-common/compare/v1.4.0...v1.5.0

## [1.4.0] - 2019-11-05

First release with full `Istanbul` support regarding parameter introductions/updates
and HF block numbers set for supported chains.

Relevant PRs:

- Added `Istanbul` block numbers for `mainnet`, `goerli` and `rinkeby`,
  PR [#68](https://github.com/ethereumjs/ethereumjs-common/pull/68)
- Added `Petersburg` and `Constantinople` fork blocks to `rinkeby`,
  PR [#71](https://github.com/ethereumjs/ethereumjs-common/pull/71)
- Added `EIP-2200` (rebalance net-metered SSTORE gas costs) parameters for `Istanbul`,
  PR [#65](https://github.com/ethereumjs/ethereumjs-common/pull/65)

Other noteworthy changes:

- Adding forks (including `Istanbul`) for `kovan`,
  PR [#70](https://github.com/ethereumjs/ethereumjs-common/pull/70)
- Fixed `kovan` genesis state,
  PR [#66](https://github.com/ethereumjs/ethereumjs-common/pull/66)

[1.4.0]: https://github.com/ethereumjs/ethereumjs-common/compare/v1.3.2...v1.4.0

## [1.3.2] - 2019-09-04

**Istanbul** Updates:

- Added gas parameters for `EIP-2200` (rebalanced net-metered SSTORE
  gas costs), PR [#65](https://github.com/ethereumjs/ethereumjs-common/pull/65)
- Renamed hardfork `blake2bRound` (-> `blake2Round`) parameter,
  PR [#63](https://github.com/ethereumjs/ethereumjs-common/pull/63)

Other Changes:

- Fixed `Kovan` genesis state,
  PR [#66](https://github.com/ethereumjs/ethereumjs-common/pull/66)

[1.3.2]: https://github.com/ethereumjs/ethereumjs-common/compare/v1.3.1...v1.3.2

## [1.3.1] - 2019-08-08

Added missing **Istanbul** gas costs for:

- ChainID opcode (EIP-1344, as base param in `hardforks/chainstart.json`)
- Blake2b precompile (EIP-2129/152)
- Calldata gas cost reduction (EIP-2028)

See PR [#58](https://github.com/ethereumjs/ethereumjs-common/pull/58).

[1.3.1]: https://github.com/ethereumjs/ethereumjs-common/compare/v1.3.0...v1.3.1

## [1.3.0] - 2019-06-18

- Add a static factory method `Custom.forCustomChain` to make working with
  custom/private chains easier.

[1.3.0]: https://github.com/ethereumjs/ethereumjs-common/compare/v1.2.1...v1.3.0

## [1.2.1] - 2019-06-03

- Added `Istanbul` HF candidate [EIP-1108](https://eips.ethereum.org/EIPS/eip-1108)
  (`DRAFT`) updated `alt_bn128` precompile gas costs (see `hardforks/istanbul.json`)

[1.2.1]: https://github.com/ethereumjs/ethereumjs-common/compare/v1.2.0...v1.2.1

## [1.2.0] - 2019-05-27

**DRAFT Istanbul Hardfork Support**

Draft support for the upcoming `Istanbul` hardfork planned for October 2019,
use `istanbul` as constructor `hardfork` parameter to activate. Parameters
relevant to new EIPs accepted for the HF will be added along subsequent `1.2.x`
releases, the finalized HF version will be released along a subsequent `1.x.0`
release (likely `1.3.0`).

See new `hardforks/istanbul.json` file as well as PR
[#51](https://github.com/ethereumjs/ethereumjs-common/pull/51).

[1.2.0]: https://github.com/ethereumjs/ethereumjs-common/compare/v1.1.0...v1.2.0

## [1.1.0] - 2019-02-04

**Petersburg Hardfork Support**

This release now supports the new `Petersburg` (aka
`constantinopleFix`) HF removing support for [EIP 1283](https://eips.ethereum.org/EIPS/eip-1283). `Petersburg` is conceptualized
within the library as a separate delta-containing HF, only removing `EIP 1283`
support and containing nothing else. It should therefore always be applied
together with the `Constantinople` HF, either by using the same block number to
update on both (`mainnet` scenario) or applying subsequently on subsequent
block numbers (`ropsten` scenario).

HF related changes (from PR [#44](https://github.com/ethereumjs/ethereumjs-common/pull/44)):

- New `hardforks/petersburg.json` HF file
- `constantinople` and `petersburg` block numbers for `ropsten` and `mainnet`
- Updated tests, new `petersburg` related tests

**Launched/Final Goerli Configuration Support**

The release now supports the final [Goerli](https://github.com/goerli/testnet)
cross-client testnet configuration.

Goerli related changes (from PR [#48](https://github.com/ethereumjs/ethereumjs-common/pull/48)):

- Updated `chains/goerli.json` configuration file (`chainId` -> 5,
  `networkId` -> 5, genesis parameters)
- HF block numbers up to `petersburg` hardfork
- Updated bootstrap nodes
- Updated `genesisStates/goerli.json` genesis state
- Test updates

**Other Changes**

- Fixed a bug in `hardforkGteHardfork()` where non-active hardforks were considered equal to `chainstart` when `onlyActive` is passed, see
  PR [#44](https://github.com/ethereumjs/ethereumjs-common/pull/44)
- Use CLI scripts from ethereumjs-config in package.json, PR
  [#43](https://github.com/ethereumjs/ethereumjs-common/pull/43)

[1.1.0]: https://github.com/ethereumjs/ethereumjs-common/compare/v1.0.0...v1.1.0

## [1.0.0] - 2019-01-23

First `TypeScript` based release of the library (for details see
PR [#38](https://github.com/ethereumjs/ethereumjs-common/pull/38)),
so release coming with type declaration files and additional type safety! 😄

### Breaking Changes

**Library Import**

`TypeScript` handles `ES6` transpilation
[a bit differently](https://github.com/Microsoft/TypeScript/issues/2719) (at the
end: cleaner) than `babel` so `require` syntax of the library slightly changes to:

```javascript
const Common = require('ethereumjs-common').default
```

**Genesis State Import/Usage**

Import path and usage API of genesis state has changed, see also the
[docs](https://github.com/ethereumjs/ethereumjs-common#genesis-states) on this,
PR [#39](https://github.com/ethereumjs/ethereumjs-common/pull/39):

```javascript
const mainnetGenesisState = require('ethereumjs-common/dist/genesisStates/mainnet')
```

Or by accessing dynamically:

```javascript
const genesisStates = require('ethereumjs-common/dist/genesisStates')
const mainnetGenesisState = genesisStates.genesisStateByName('mainnet')
const mainnetGenesisState = genesisStates.genesisStateById(1) // alternative via network Id
```

**Removed `hybridCasper` (draft) hardfork**

Not likely that anyone has used this, but just in case:
The once anticipated `hybridCasper` (draft) hardfork has been removed from the
list of hardforks, see PR [#37](https://github.com/ethereumjs/ethereumjs-common/pull/37)

[1.0.0]: https://github.com/ethereumjs/ethereumjs-common/compare/v0.6.1...v1.0.0

## [0.6.1] - 2018-11-28

- Experimental support for the [Goerli](https://github.com/goerli/testnet) cross-client `PoA` testnet (`chains/goerli.json`), see PR [#31](https://github.com/ethereumjs/ethereumjs-common/pull/31)
- Unified hex-prefixing (so always prefixing with `0x`) of account addresses in genesis files (fixes an issue with state root computation on other libraries), see PR [#32](https://github.com/ethereumjs/ethereumjs-common/issues/32)

[0.6.1]: https://github.com/ethereumjs/ethereumjs-common/compare/v0.6.0...v0.6.1

## [0.6.0] - 2018-10-11

Parameter support for the `Constantinople` hardfork (see `hardforks/constantinople.json`):

- Added `SSTORE` gas/refund prices (`EIP-1283`), PR [#27](https://github.com/ethereumjs/ethereumjs-common/pull/27)
- Added Block Reward Adjustment (`EIP-1234`), PR [#26](https://github.com/ethereumjs/ethereumjs-common/pull/26)

[0.6.0]: https://github.com/ethereumjs/ethereumjs-common/compare/v0.5.0...v0.6.0

## [0.5.0] - 2018-08-27

- Introduces **support for private chains** by allowing to pass a custom dictionary as the `chain` parameter
  in the constructor or the `setChain()` method as an alternative to just passing one of the predefined
  `chain` `String` names (e.g. `mainnet`, `ropsten`), PR [#24](https://github.com/ethereumjs/ethereumjs-common/pull/24)

[0.5.0]: https://github.com/ethereumjs/ethereumjs-common/compare/v0.4.1...v0.5.0

## [0.4.1] - 2018-08-13

- Added `timestamp` field to genesis definitions in chain files, set for `Rinkeby` and `null` for other chains, PR [#21](https://github.com/ethereumjs/ethereumjs-common/pull/21)
- Updated `Ropsten` bootstrap nodes, PR [#20](https://github.com/ethereumjs/ethereumjs-common/pull/20)

[0.4.1]: https://github.com/ethereumjs/ethereumjs-common/compare/v0.4.0...v0.4.1

## [0.4.0] - 2018-06-20

- Remove leftover ...Gas postfix for some gas prices (e.g. `ecAddGas` -> `ecAdd`) to
  be consistent with overall gas price naming

[0.4.0]: https://github.com/ethereumjs/ethereumjs-common/compare/v0.3.1...v0.4.0

## [0.3.1] - 2018-05-28

- Added two alias functions `activeOnBlock()` and `gteHardfork()` when hardfork is set for convenience, PR [#15](https://github.com/ethereumjs/ethereumjs-common/pull/15)
- Added option to dynamically choose genesis state (see `README`), PR [#15](https://github.com/ethereumjs/ethereumjs-common/pull/15)

[0.3.1]: https://github.com/ethereumjs/ethereumjs-common/compare/v0.3.0...v0.3.1

## [0.3.0] - 2018-05-25

- Allow functions like `hardforkIsActiveOnBlock()` - where hardfork is provided as param - also to be run on hardfork set for greater flexibility/comfort, PR [#13](https://github.com/ethereumjs/ethereumjs-common/pull/13)
- New `hardforkGteHardfork()` method for HF order comparisons, PR [#13](https://github.com/ethereumjs/ethereumjs-common/pull/13)

[0.3.0]: https://github.com/ethereumjs/ethereumjs-common/compare/v0.2.0...v0.3.0

## [0.2.0] - 2018-05-14

- New optional initialization parameter `allowedHardforks`, this allows for cleaner client
  library implementations by preventing undefined behaviour, PR [#10](https://github.com/ethereumjs/ethereumjs-common/pull/10)
- Added `activeHardfork()` function to get latest active HF for chain or block, PR [#11](https://github.com/ethereumjs/ethereumjs-common/pull/11)

[0.2.0]: https://github.com/ethereumjs/ethereumjs-common/compare/v0.1.1...v0.2.0

## [0.1.1] - 2018-05-09

- Remove dynamic require to prevent browserify issue, PR [#8](https://github.com/ethereumjs/ethereumjs-common/pull/8)

[0.1.1]: https://github.com/ethereumjs/ethereumjs-common/compare/v0.1.0...v0.1.1

## [0.1.0] - 2018-05-09

- Initial version, this library succeeds the [ethereum/common](https://github.com/ethereumjs/common/issues/12)
  library, being more future-proof through a better structured design

Features:

- Easy chain-/HF-based parameter access
- No parameter changes on library updates (`c.param('gasPrices', 'ecAddGas', 'byzantium')` will always return the same value)
- Ease experimentation/research by allowing to include future HF parameters (already included as draft: `constantinople` and `hybridCasper`) without breaking current installations
- Improved structure for parameter access (mainly through topics like `gasPrices`, `pow`, `sharding`) for better readability/developer overview
- See [README](https://github.com/ethereumjs/ethereumjs-common) and [API Docs](https://github.com/ethereumjs/ethereumjs-common/blob/master/docs/index.md) for a more in-depth feature overview and usage instructions

[0.1.0]: https://github.com/ethereumjs/ethereumjs-common/compare/6d0df89...v0.1.0
