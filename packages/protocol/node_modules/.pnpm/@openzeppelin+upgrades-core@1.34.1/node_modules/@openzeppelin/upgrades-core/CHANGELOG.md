# Changelog

## 1.34.1 (2024-06-18)

- Fix unexpected validation error when function parameter has internal function pointer. ([#1038](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/1038))

## 1.34.0 (2024-06-12)

- Fix storage layout comparison for function types, disallow internal functions in storage. ([#1032](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/1032))

## 1.33.1 (2024-04-25)

- Fix Hardhat compile error when variable has whitespace before semicolon. ([#1020](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/1020))

## 1.33.0 (2024-04-24)

- Enable changing default network files directory with environment variable. ([#1011](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/1011))

## 1.32.6 (2024-04-16)

- This plugin is now compiled with TypeScript v5. ([#760](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/760))
- Fix Hardhat compile error when referencing a constant within a struct definition. ([#1009](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/1009))

## 1.32.5 (2024-02-21)

- Add support for Holesky testnet to manifest file names. ([#974](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/974))

## 1.32.4 (2024-01-30)

- Add support for OP Sepolia to manifest file names. ([#963](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/963))
- Add support for Base networks to manifest file names. ([#965](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/965))
- Fix error when renaming network file and using a separate filesystem. ([#964](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/964))

## 1.32.3 (2024-01-16)

- CLI: Improve checks for build info file settings. ([#958](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/958))

## 1.32.2 (2023-12-21)

- Fix manifest error when connecting to an Anvil dev network. ([#950](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/950))

## 1.32.1 (2023-12-14)

- CLI: Fix ambiguous name error when passing in fully qualified contract names. ([#944](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/944))

## 1.32.0 (2023-12-11)

- Support deploying proxies from OpenZeppelin Contracts 5.0. ([#919](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/919))
- Fix address clash when redeploying implementation. ([#939](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/939))

## 1.31.3 (2023-11-28)

- Fix Hardhat compile errors when contracts have overloaded functions or standalone NatSpec documentation. ([#918](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/918))

## 1.31.2 (2023-11-28)

- Fix `upgradeProxy` in Hardhat from an implementation that has a fallback function and is not using OpenZeppelin Contracts 5.0. ([#926](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/926))

## 1.31.1 (2023-11-01)

- CLI: Throw error if `--requireReference` and `--unsafeSkipStorageCheck` are both enabled. ([#913](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/913))

## 1.31.0 (2023-10-23)

- CLI: Add `--requireReference` option. ([#900](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/900))
- CLI: Simplify summary message when using `--contract`. ([#905](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/905))

## 1.30.1 (2023-10-11)

- Fix Hardhat compile error when using Solidity 0.5.x. ([#892](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/892))

## 1.30.0 (2023-09-27)

- Support new upgrade interface in OpenZeppelin Contracts 5.0. ([#883](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/883))
- Add validations for namespaced storage layout. ([#876](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/876))
- Deprecate low-level API. Use [CLI or high-level API](https://docs.openzeppelin.com/upgrades-plugins/1.x/api-core) instead.

## 1.29.0 (2023-09-19)

- Support implementations with upgradeTo or upgradeToAndCall. ([#880](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/880))

## 1.28.0 (2023-08-03)

- Support `contract` and `reference` options for CLI. ([#856](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/856))

## 1.27.3 (2023-07-12)

- Support user-defined value types in mappings. ([#844](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/844))

## 1.27.2 (2023-07-10)

- Allow assignment of immutable variables if the `state-variable-immutable` override is present. ([#838](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/838))

## 1.27.1 (2023-06-15)

- Update recommended Foundry config for CLI. ([#818](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/818))

## 1.27.0 (2023-06-14)

- Add CLI for upgrade safety checks. ([#807](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/807))

## 1.26.2 (2023-05-12)

- Add missing file in package. ([#797](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/797))

## 1.26.1 (2023-05-12)

- Use proxies from OpenZeppelin Contracts 4.8.3. ([#795](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/795))

## 1.26.0 (2023-05-08)

- Enable using OpenZeppelin Platform for deployments. ([#763](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/763))

**Note**: OpenZeppelin Platform is currently in beta and functionality related to it is subject to change.

## 1.25.0 (2023-04-26)

- Add support for Arbitrum to manifest file names. ([#770](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/770))
- Add support for Sepolia testnet to manifest file names. ([#766](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/766))
- Support `prepareUpgrade` from an implementation address. ([#777](https://github.com/OpenZeppelin/openzeppelin-upgrades/issues/777))

## 1.24.1 (2023-03-02)

- Remove test contracts from source code verification. ([#751](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/751))

## 1.24.0 (2023-02-14)

- Support Hardhat tests in --parallel mode when using Hardhat version 2.12.3 or later. ([#726](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/726))
- Support Hardhat forked networks when using Hardhat version 2.12.3 or later. ([#726](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/726))
- Add Foundry's anvil as a development network. ([#744](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/744))

## 1.23.0 (2023-02-09)

- Support storage gaps named with `__gap_*`. ([#732](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/732))
- Improve detection of storage gap usage. ([#731](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/731))
- Add support for Optimism to manifest filenames. ([#740](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/740))

## 1.22.0 (2023-01-31)

- Add support for Binance Smart Chain to manifest file names. ([#729](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/729))
- Improve compilation performance for validations. ([#724](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/724))

## 1.21.0 (2023-01-18)

- Add support for celo and celo-alfajores to manifest file names. ([#710](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/710))
- Only consider errors from functions in use. Validate free functions. ([#702](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/702))
- Improve handling of NatSpec comments. ([#717](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/717)) ([#720](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/720))
- Fix runtime type error. ([#721](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/721))

## 1.20.6 (2022-12-15)

- Fix display issue in storage layout reports. ([#699](https://github.com/OpenZeppelin/openzeppelin-upgrades/issues/699))

## 1.20.5 (2022-11-25)

- Fix incompatible type error when upgrading from mapping with strings ([#689](https://github.com/OpenZeppelin/openzeppelin-upgrades/issues/689))
- Support retype from contract, interface, struct or enum to address. ([#687](https://github.com/OpenZeppelin/openzeppelin-upgrades/issues/687))

## 1.20.4 (2022-11-03)

- Support multiple contracts with same name. ([#263](https://github.com/OpenZeppelin/openzeppelin-upgrades/issues/263))

## 1.20.3 (2022-11-02)

- Use underlying type of user defined value types in the storage layout for layout comparison. ([#682](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/682))

## 1.20.2 (2022-10-26)

- Add underlying type of user defined value types in the storage layout. ([#681](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/681))

## 1.20.1 (2022-10-04)

- Fix parsing of renamed/retyped annotations. ([#666](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/666))
- Fix treatment of type ids for user defined value types. ([#671](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/671))

## 1.20.0 (2022-09-27)

- Support user defined value types. ([#658](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/658))
- Include solc version in storage layouts in manifest files. ([#662](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/662))

## 1.19.1 (2022-09-10)

- Fix missing module. ([#652](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/652))

## 1.19.0 (2022-09-10)

- Add support for avalanche and avalanche-fuji to manifest file names. ([#622](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/622))
- Support storage layout gaps. ([#276](https://github.com/OpenZeppelin/openzeppelin-upgrades/issues/276))

## 1.18.0 (2022-08-08)

- Provide granular functions to allow more customizable deployments. ([#580](https://github.com/OpenZeppelin/openzeppelin-upgrades/issues/580))

## 1.17.0 (2022-07-26)

- Remove BN.js in favor of native BigInt. ([#602](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/602))
- Add support for additional network names in network manifest. ([#547](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/547))

## 1.16.1 (2022-06-16)

- Fix VM execution error in `proposeUpgrade` with Gnosis Chain. ([#597](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/597))

## 1.16.0 (2022-06-16)

- Return ethers transaction response with `proposeUpgrade`. ([#554](https://github.com/OpenZeppelin/openzeppelin-upgrades/issues/554))

## 1.15.0 (2022-05-31)

- Support `unsafeSkipStorageCheck` option in `ValidationOptions`. ([#566](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/566))
- Support verifying a proxy on Etherscan using Hardhat. ([#579](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/579))

## 1.14.2 (2022-04-18)

- Fix handling of rename annotations when unchanged from one version to the next. ([#558](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/558))

## 1.14.1 (2022-03-16)

- Fix interference with other Hardhat plugins that use the AST. ([#541](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/541))

## 1.14.0 (2022-03-14)

- Add support for function types in storage layout. ([#529](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/529))
- Add ability to change a variable type or name through `/// @custom:oz-renamed-from abc` and `/// @custom:oz-retyped-from bool`. ([#531](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/531))

## 1.13.1 (2022-03-08)

- Fix bug that caused missing members when using solc storageLayout. ([#534](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/534))

## 1.13.0 (2022-03-01)

- Remove `admin.deployTransaction` field written to network manifest. ([#510](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/510))
- Add `forceImport` function to import an existing proxy or beacon. ([#517](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/517))
- Extract and store more detailed information about storage layout. ([#519](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/519))

## 1.12.0 (2022-01-31)

- Add options `timeout` and `pollingInterval`. ([#55](https://github.com/OpenZeppelin/openzeppelin-upgrades/issues/55))

## 1.11.1 (2022-01-20)

- Fix error when proposing upgrade to Infura. ([#503](https://github.com/OpenZeppelin/openzeppelin-upgrades/issues/503))

## 1.11.0 (2022-01-12)

- Add support for beacon proxies. ([#342](https://github.com/OpenZeppelin/openzeppelin-upgrades/issues/342))

## 1.10.0 (2021-10-22)

- Infer whether a proxy should be UUPS or Transparent. ([#441](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/441))
- Add a standalone interface to the core. ([#442](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/442))
- Add implementation constructor arguments to initialize immutable variables. ([#433](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/433))

## 1.9.2 (2021-09-17)

- Fix a bug where libraries used transitively were not considered for safety checks.

## 1.9.1 (2021-09-15)

- Silence all warnings when using `silenceWarnings`.

## 1.9.0 (2021-08-25)

- Add `getBeaconAddress`. ([#413](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/413))

## 1.8.1 (2021-08-06)

- Add support for function types.

## 1.8.0 (2021-06-29)

- Add option `unsafeAllowRenames` to configure storage layout check to allow variable renaming.

## 1.7.6 (2021-05-25)

- Fix recognition of value types in storage layouts.

## 1.7.5 (2021-05-22)

- Fix exception when upgrading mapping key to enum.

## 1.7.4 (2021-05-22)

- Fix handling of functions with struct or mapping storage pointer arguments.

## 1.7.3 (2021-05-13)

- Simplify clean up of manifest data before storing to disk.

## 1.7.2 (2021-05-13)

- Fix bug when the project uses external libraries.

## 1.7.1 (2021-05-03)

- Use `web3_clientVersion` to better detect development networks.

## 1.7.0 (2021-04-29)

- Add support for UUPS proxies. ([#315](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/315))
- Fix parsing of NatSpec `@custom:oz-upgrades-unsafe-allow` when included in a `/**`-style comment.

## 1.6.0 (2021-04-14)
- Add `unsafeAllow` as a new field in `ValidationOptions`, which can be used as a manual override to silence any type of validation error. For example, `opts = { unsafeAllow: ['external-library-linking', 'delegatecall'] }` will silence the corresponding checks. ([#320](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/320))
- Custom NatSpec comments can disable error checks directly from the Solidity code. See `core/contracts/test/ValidationNatspec.sol` for example usage of these NatSpec comments. Note: this requires Solidity >=0.8.2. ([#320](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/320))
- Fix a bug with library placeholders when hashing contract source code. ([#320](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/320))

## 1.5.1 (2021-02-24)

- Add support for enum keys in mappings. ([#301](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/301))

## 1.5.0 (2021-01-27)

- Add support for structs and enums. ([#261](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/261))
- Enable optimizations when compiling proxies.

## 1.4.4 (2021-01-14)

- Fix `InvalidDeployment` error on some networks. ([#282](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/282))

## 1.4.3 (2020-12-21)

- Fix a type error caused by duplicate contract names in Truffle.

## 1.4.2 (2020-12-16)

- Fix an error in the `unsafeAllowCustomTypes` flag that would cause other storage layout incompatibilities to be ignored. ([#259](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/259))

Users of this flag are advised to update to this version.

## 1.4.1 (2020-11-30)

- Fix a problem in deployment logic when used with Hyperledger Besu. ([#244](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/244))

## 1.4.0 (2020-11-24)

- Add `silenceWarnings` to emit a single warning and silence all subsequent ones. ([#228](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/228))

## 1.3.1 (2020-11-13)

- Add missing artifact files in the package.

## 1.3.0 (2020-11-13)

- Support Hardhat. ([#214](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/214))

## 1.2.0 (2020-10-16)

- Support new flag `unsafeAllowLinkedLibraries` to allow deployment of contracts with linked libraries. ([#182](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/182))

## 1.1.1 (2020-10-08)

- Fix OpenZeppelin CLI migration for projects that were initialized with ZeppelinOS. ([#193](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/193))

## 1.1.0 (2020-09-18)

- Add ability to migrate from OpenZeppelin CLI. ([#143](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/143))

## 1.0.2 (2020-09-16)

- Fix false positive variable initialization check in Solidity 0.7.1. ([#171](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/171))

## 1.0.1 (2020-09-01)

- Include `src` directory in npm packages. ([#150](https://github.com/OpenZeppelin/openzeppelin-upgrades/pull/150))
