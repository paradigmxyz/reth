# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/) 
(modification: no type change headlines) and this project adheres to 
[Semantic Versioning](http://semver.org/spec/v2.0.0.html).


## [2.6.0] - 2019-02-07

**Petersburg Support**

Support for the `Petersburg` (aka `constantinopleFix`) hardfork by integrating
`Petersburg` ready versions of associated libraries, see also 
PR [#433](https://github.com/ethereumjs/ethereumjs-vm/pull/433):

- `ethereumjs-common` (chain and HF logic and helper functionality) [v1.1.0](https://github.com/ethereumjs/ethereumjs-common/releases/tag/v1.1.0)
- `ethereumjs-blockchain` [v3.4.0](https://github.com/ethereumjs/ethereumjs-blockchain/releases/tag/v3.4.0)
- `ethereumjs-block` [v2.2.0](https://github.com/ethereumjs/ethereumjs-block/releases)

To instantiate the VM with `Petersburg` HF rules set the `opts.hardfork`
constructor parameter to `petersburg`. This will run the VM on the new
Petersburg rules having removed the support for 
[EIP 1283](https://eips.ethereum.org/EIPS/eip-1283).

**Goerli Readiness**

The VM is now also ready to execute on blocks from the final version of the
[Goerli](https://github.com/goerli/testnet) cross-client testnet and can
therefore be instantiated with `opts.chain` set to `goerli`. 

**Bug Fixes**

- Fixed mixed `sync`/`async` functions in `cache`,
  PR [#422](https://github.com/ethereumjs/ethereumjs-vm/pull/422)
- Fixed a bug in `setStateroot` and caching by clearing the `stateManager` cache
  after setting the state root such that stale values are not returned,
  PR [#420](https://github.com/ethereumjs/ethereumjs-vm/pull/420)
- Fixed cache access on the hooked VM (*deprecated*), 
  PR [#434](https://github.com/ethereumjs/ethereumjs-vm/pull/434)

**Refactoring**

Following changes might be relevant for you if you are hotfixing/monkey-patching
on parts of the VM:

- Moved `bloom` to its own directory, 
  PR [#429](https://github.com/ethereumjs/ethereumjs-vm/pull/429)
- Moved `opcodes`, `opFns` and `logTable` to `lib/vm`,
  PR [#425](https://github.com/ethereumjs/ethereumjs-vm/pull/425)
- Converted `Bloom` to `ES6` class, 
  PR [#428](https://github.com/ethereumjs/ethereumjs-vm/pull/428)
- Converted `Cache` to `ES6` class, added unit tests,
  PR [427](https://github.com/ethereumjs/ethereumjs-vm/pull/427)

[2.6.0]: https://github.com/ethereumjs/ethereumjs-vm/compare/v2.5.1...v2.6.0

## [2.5.1] - 2019-01-19

### Features

- Added `memoryWordCount` to the `step` event object,
  PR [#405](https://github.com/ethereumjs/ethereumjs-vm/pull/405)

### Bug Fixes

- Fixed a bug which caused an overwrite of the passed state trie (`opts.state`)
  when instantiating the library with the `opts.activatePrecompiles` option,
  PR [#415](https://github.com/ethereumjs/ethereumjs-vm/pull/415)
- Fixed error handling in `runCode` (in case `loadContract` fails),
  PR [#408](https://github.com/ethereumjs/ethereumjs-vm/pull/408)
- Fixed a bug in the `StateManager.generateGenesis()` function,
  PR [#400](https://github.com/ethereumjs/ethereumjs-vm/pull/400)
  
### Tests

- Upgraded `ethereumjs-blockchain` and `level` for test runs,
  PR [#414](https://github.com/ethereumjs/ethereumjs-vm/pull/414)
- Fixed issue when running code coverage on PRs from forks,
  PR [#402](https://github.com/ethereumjs/ethereumjs-vm/pull/402)


[2.5.1]: https://github.com/ethereumjs/ethereumjs-vm/compare/v2.5.0...v2.5.1

## [2.5.0] - 2018-11-21

This is the first release of the VM with full support for all ``Constantinople`` EIPs. It further comes along with huge improvements on consensus conformity and introduces the ``Beta`` version of a new ``StateManager`` API.

### Constantinople Support

For running the VM with ``Constantinople`` hardfork rules, set the [option](https://github.com/ethereumjs/ethereumjs-vm/blob/master/docs/index.md#vm) in the ``VM`` constructor ``opts.hardfork`` to ``constantinople``. Supported hardforks are ``byzantium`` and ``constantinople``, ``default`` setting will stay on ``byzantium`` for now but this will change in a future release.

Changes related to Constantinople:

- EIP 1283 ``SSTORE``, see PR [#367](https://github.com/ethereumjs/ethereumjs-vm/pull/367)
- EIP 1014 ``CREATE2``, see PR [#329](https://github.com/ethereumjs/ethereumjs-vm/pull/329)
- EIP 1052 ``EXTCODEHASH``, see PR [#324](https://github.com/ethereumjs/ethereumjs-vm/pull/324)
- Constantinople ready versions of [ethereumjs-block](https://github.com/ethereumjs/ethereumjs-block/releases/tag/v2.1.0) and [ethereumjs-blockchain](https://github.com/ethereumjs/ethereumjs-blockchain/releases/tag/v3.3.0) dependencies (difficulty bomb delay), see PRs [#371](https://github.com/ethereumjs/ethereumjs-vm/pull/371), [#325](https://github.com/ethereumjs/ethereumjs-vm/pull/325)

### Consensus Conformity

This release is making a huge leap forward regarding consensus conformity, and even if you are not interested in ``Constantinople`` support at all, you should upgrade just for this reason. Some context: we couldn't run blockchain tests for a long time on a steady basis due to performance constraints and when we re-triggered a test run after quite some time with PR [#341](https://github.com/ethereumjs/ethereumjs-vm/pull/341) the result was a bit depressing with over 300 failing tests. Thanks to joined efforts from the community and core team members we could bring this down far quicker than expected and this is the first release for a long time which practically comes with complete consensus conformity - with just three recently added tests failing (see ``skipBroken`` list in ``tests/tester.js``) and otherwise passing all blockchain tests and all state tests for both ``Constantinople`` and ``Byzantium`` rules. 🏆 🏆 🏆

Consensus Conformity related changes:

- Reset ``selfdestruct`` on ``REVERT``, see PR [#392](https://github.com/ethereumjs/ethereumjs-vm/pull/392)
- Undo ``Bloom`` filter changes from PR [#295](https://github.com/ethereumjs/ethereumjs-vm/pull/295), see PR [#384](https://github.com/ethereumjs/ethereumjs-vm/pull/384)
- Fixes broken ``BLOCKHASH`` opcode, see PR [#381](https://github.com/ethereumjs/ethereumjs-vm/pull/381)
- Fix failing blockchain test ``GasLimitHigherThan2p63m1``, see PR [#380](https://github.com/ethereumjs/ethereumjs-vm/pull/380)
- Stop adding ``account`` to ``cache`` when checking if it is empty, see PR [#375](https://github.com/ethereumjs/ethereumjs-vm/pull/375)

### State Manager Interface

The ``StateManager`` (``lib/stateManager.js``) - providing a high-level interface to account and contract data from the underlying state trie structure - has been completely reworked and there is now a close-to-being finalized API (currently marked as ``Beta``) coming with its own [documentation](https://github.com/ethereumjs/ethereumjs-vm/blob/master/docs/stateManager.md).

This comes along with larger refactoring work throughout more-or-less the whole code base and the ``StateManager`` now completely encapsulates the trie structure and the cache backend used, see issue [#268](https://github.com/ethereumjs/ethereumjs-vm/issues/268) and associated PRs for reference. This will make it much easier in the future to bring along an own state manager serving special needs (optimized for memory and performance, run on mobile,...) by e.g. using a different trie implementation, cache or underlying storage or database backend.

We plan to completely separate the currently still integrated state manager into its own repository in one of the next releases, this will then be a breaking ``v3.0.0`` release. Discussion around a finalized interface (we might e.g. drop all genesis-releated methods respectively methods implemented in the ``DefaultStateManager``) is still ongoing and you are very much invited to jump in and articulate your needs, just take e.g. the issue mentioned above as an entry point.

Change related to the new ``StateManager`` interface:


- ``StateManager`` interface simplification, see PR [#388](https://github.com/ethereumjs/ethereumjs-vm/pull/388)
- Make ``StateManager`` cache and trie private, see PR [#385](https://github.com/ethereumjs/ethereumjs-vm/pull/385)
- Remove vm accesses to ``StateManager`` ``trie`` and ``cache``, see PR [#376](https://github.com/ethereumjs/ethereumjs-vm/pull/376)
- Remove explicit direct cache interactions, see PR [#366](https://github.com/ethereumjs/ethereumjs-vm/pull/366)
- Remove contract specific commit, see PR [#335](https://github.com/ethereumjs/ethereumjs-vm/pull/335)
- Fixed incorrect references to ``trie`` in tests, see PR [#345](https://github.com/ethereumjs/ethereumjs-vm/pull/345)
- Added ``StateManager`` API documentation, see PR [#393](https://github.com/ethereumjs/ethereumjs-vm/pull/393)


### New Features

- New ``emitFreeLogs`` option, allowing any contract to emit an unlimited quantity of events without modifying the block gas limit (default: ``false``) which can be used in debugging contexts, see PRs [#378](https://github.com/ethereumjs/ethereumjs-vm/pull/378), [#379](https://github.com/ethereumjs/ethereumjs-vm/pull/379)

### Testing and Documentation

Beyond the reintegrated blockchain tests there is now a separate test suite to test the API of the library, see ``tests/api``. This should largely reduce the risk of introducing new bugs on the API level on future changes, generally ease the development process by being able to develop against the specific tests and also allows using the tests as a reference for examples on how to use the API.

On the documentation side the API documentation has also been consolidated and there is now a unified and auto-generated [API documentation](https://github.com/ethereumjs/ethereumjs-vm/blob/master/docs/index.md) (previously being manually edited (and too often forgotten) in ``README``).

- Added API tests for ``index.js``, ``StateManager``, see PR [#364](https://github.com/ethereumjs/ethereumjs-vm/pull/364)
- Added API Tests for ``runJit`` and ``fakeBlockchain``, see PR [#331](https://github.com/ethereumjs/ethereumjs-vm/pull/331)
- Added API tests for ``runBlockchain``, see PR [#336](https://github.com/ethereumjs/ethereumjs-vm/pull/336)
- Added ``runBlock`` API tests, see PR [#360](https://github.com/ethereumjs/ethereumjs-vm/pull/360)
- Added ``runTx`` API tests, see PR [#352](https://github.com/ethereumjs/ethereumjs-vm/pull/352)
- Added API Tests for the ``Bloom`` module, see PR [#330](https://github.com/ethereumjs/ethereumjs-vm/pull/330)
- New consistent auto-generated [API documentation](https://github.com/ethereumjs/ethereumjs-vm/blob/master/docs/index.md), see PR [#377](https://github.com/ethereumjs/ethereumjs-vm/pull/377)
- Blockchain tests now run by default on CI, see PR [#374](https://github.com/ethereumjs/ethereumjs-vm/pull/374)
- Switched from ``istanbul`` to ``nyc``, see PR [#334](https://github.com/ethereumjs/ethereumjs-vm/pull/334)
- Usage of ``sealEngine`` in blockchain tests, see PR [#373](https://github.com/ethereumjs/ethereumjs-vm/pull/373)
- New ``tap-spec`` option to get a formatted test run result summary, see [README](https://github.com/ethereumjs/ethereumjs-vm#running-tests-with-a-reporterformatter), see PR [#363](https://github.com/ethereumjs/ethereumjs-vm/pull/363)
- Updates/fixes on the JSDoc comments, see PRs [#362](https://github.com/ethereumjs/ethereumjs-vm/pull/362), [#361](https://github.com/ethereumjs/ethereumjs-vm/pull/361)

### Bug Fixes and Maintenance

Some bug fix and maintenance updates:

- Fix error handling in ``fakeBlockChain``, see PR [#320](https://github.com/ethereumjs/ethereumjs-vm/pull/320)
- Update of ``ethereumjs-util`` to [v6.0.0](https://github.com/ethereumjs/ethereumjs-util/releases/tag/v6.0.0), see PR [#369](https://github.com/ethereumjs/ethereumjs-vm/pull/369)


### Thank You

Special thanks to:

- @mattdean-digicatapult for his indefatigable work on the new StateManager interface and for fixing a large portion of the failing blockchain tests
- @rmeissner for the work on Constantinople 
- @vpulim for jumping in so quickly and doing a reliable ``SSTORE`` implementation within 4 days
- @s1na for the new API test suite

Beyond this release contains contributions from the following people:
@jwasinger, @Agusx1211, @HolgerD77, @danjm, @whymarrh, @seesemichaelj, @kn

Thank you all very much, and thanks @axic for keeping an ongoing eye on overall library quality!

[2.5.0]: https://github.com/ethereumjs/ethereumjs-vm/compare/v2.4.0...v2.5.0

## [2.4.0] - 2018-07-27

With the ``2.4.x`` release series we now start to gradually add ``Constantinople`` features with the
bitwise shifting instructions from [EIP 145](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-145.md)
making the start being introduced in the ``v2.4.0`` release.

Since both the scope of the ``Constantinople`` hardfork as well as the state of at least some of the EIPs
to be included are not yet finalized, this is only meant for ``EXPERIMENTAL`` purposes, e.g. for developer
tools to give users early access and make themself familiar with dedicated features.

Once scope and EIPs from ``Constantinople`` are final we will target a ``v2.5.0`` release which will officially
introduce ``Constantinople`` support with all the changes bundled together.

Note that from this release on we also introduce new ``chain`` (default: ``mainnet``) and ``hardfork`` 
(default: ``byzantium``) initialization parameters, which make use of our new [ethereumjs-common](https://github.com/ethereumjs/ethereumjs-common) library and in the future will allow
for parallel hardfork support from ``Byzantium`` onwards.

Since ``hardfork`` default might be changed or dropped in future releases, you might want to explicitly
set this to ``byzantium`` on your next update to avoid future unexpected behavior.

All the changes from this release:

**FEATURES/FUNCTIONALITY**

- Improved chain and fork support, see PR [#304](https://github.com/ethereumjs/ethereumjs-vm/pull/304)
- Support for the ``Constantinople`` bitwise shifiting instructions ``SHL``, ``SHR`` and ``SAR``, see PR [#251](https://github.com/ethereumjs/ethereumjs-vm/pull/251)
- New ``newContract`` event which can be used to do interrupting tasks on contract/address creation, see PR [#306](https://github.com/ethereumjs/ethereumjs-vm/pull/306)
- Alignment of behavior of bloom filter hashing to go along with mainnet compatible clients *BREAKING*, see PR [#295](https://github.com/ethereumjs/ethereumjs-vm/pull/295) 

**UPDATES/TESTING**

- Usage of the latest ``rustbn.js`` API, see PR [#312](https://github.com/ethereumjs/ethereumjs-vm/pull/312)
- Some cleanup in precompile error handling, see PR [#318](https://github.com/ethereumjs/ethereumjs-vm/pull/318)
- Some cleanup for ``StateManager``, see PR [#266](https://github.com/ethereumjs/ethereumjs-vm/pull/266)
- Renaming of ``util.sha3`` usages to ``util.keccak256`` and bump ``ethereumjs-util`` to ``v5.2.0`` (you should do to if you use ``ethereumjs-util``)
- Parallel testing of the``Byzantium`` and ``Constantinople`` state tests, see PR [#317](https://github.com/ethereumjs/ethereumjs-vm/pull/317)
- For lower build times our CI configuration now runs solely on ``CircleCI`` and support for ``Travis`` have been dropped, see PR [#316](https://github.com/ethereumjs/ethereumjs-vm/pull/316)

**BUG FIXES**

- Programmatic runtime errors in the VM execution context (within an opcode) are no longer absorbed and displayed as a VMError but explicitly thrown, allowing for easier discovery of implementation bugs, see PR [#307](https://github.com/ethereumjs/ethereumjs-vm/pull/307)
- Fix of the ``Bloom.check()`` method not working properly, see PR [#311](https://github.com/ethereumjs/ethereumjs-vm/pull/311)
- Fix a bug when ``REVERT`` is used within a ``CREATE`` context, see PR [#297](https://github.com/ethereumjs/ethereumjs-vm/pull/297)
- Fix a bug in ``FakeBlockChain`` error handing, see PR [#320](https://github.com/ethereumjs/ethereumjs-vm/pull/320)

[2.4.0]: https://github.com/ethereumjs/ethereumjs-vm/compare/v2.3.5...v2.4.0

## [2.3.5] - 2018-04-25

- Fixed ``BYTE`` opcode return value bug, PR [#293](https://github.com/ethereumjs/ethereumjs-vm/pull/293)
- Clean up touched-accounts management in ``StateManager``, PR [#287](https://github.com/ethereumjs/ethereumjs-vm/pull/287)
- New ``stateManager.copy()`` function, PR [#276](https://github.com/ethereumjs/ethereumjs-vm/pull/276)
- Updated Circle CI configuration to 2.0 format, PR [#292](https://github.com/ethereumjs/ethereumjs-vm/pull/292)

[2.3.5]: https://github.com/ethereumjs/ethereumjs-vm/compare/v2.3.4...v2.3.5

## [2.3.4] - 2018-04-06

- Support of external statemanager in VM constructor (experimental), PR [#264](https://github.com/ethereumjs/ethereumjs-vm/pull/264)
- ``ES5`` distribution on npm for better toolchain compatibility, PR [#281](https://github.com/ethereumjs/ethereumjs-vm/pull/281)
- ``allowUnlimitedContractSize`` VM option for debugging purposes, PR [#282](https://github.com/ethereumjs/ethereumjs-vm/pull/282)
- Added ``gasRefund`` to transaction results, PR [#284](https://github.com/ethereumjs/ethereumjs-vm/pull/284)
- Test coverage / coveralls support for the library, PR [#270](https://github.com/ethereumjs/ethereumjs-vm/pull/270)
- Properly calculate totalgas for large return values, PR [#275](https://github.com/ethereumjs/ethereumjs-vm/pull/275)
- Improve iterateVm check output after step hook, PR [#279](https://github.com/ethereumjs/ethereumjs-vm/pull/279)

[2.3.4]: https://github.com/ethereumjs/ethereumjs-vm/compare/v2.3.3...v2.3.4

## [2.3.3] - 2018-02-02

- Reworked memory expansion/access for opcodes, PR [#174](https://github.com/ethereumjs/ethereumjs-vm/pull/174) (fixes consensus bugs on
  large numbers >= 53 bit for opcodes using memory location)
- Keep stack items as bn.js instances (arithmetic performance increases), PRs [#159](https://github.com/ethereumjs/ethereumjs-vm/pull/159), [#254](https://github.com/ethereumjs/ethereumjs-vm/pull/254) and [#256](https://github.com/ethereumjs/ethereumjs-vm/pull/256)
- More consistent VM error handling, PR [#219](https://github.com/ethereumjs/ethereumjs-vm/pull/219)
- Validate stack items after operations, PR [#222](https://github.com/ethereumjs/ethereumjs-vm/pull/222)
- Updated ``ethereumjs-util`` dependency from ``4.5.0`` to ``5.1.x``, PR [#241](https://github.com/ethereumjs/ethereumjs-vm/pull/241)
- Fixed child contract deletion bug, PR [#246](https://github.com/ethereumjs/ethereumjs-vm/pull/246)
- Fixed a bug associated with direct stack usage, PR [#240](https://github.com/ethereumjs/ethereumjs-vm/pull/240)
- Fix error on large return fees, PR [#235](https://github.com/ethereumjs/ethereumjs-vm/pull/235)
- Various bug fixes

[2.3.3]: https://github.com/ethereumjs/ethereumjs-vm/compare/v2.3.2...v2.3.3

## [2.3.2] - 2017-10-29
- Better handling of ``rustbn.js`` exceptions
- Fake (default if non-provided) blockchain fixes
- Testing improvements (separate skip lists)
- Minor optimizations and bug fixes

[2.3.2]: https://github.com/ethereumjs/ethereumjs-vm/compare/v2.3.1...v2.3.2

## [2.3.1] - 2017-10-11
- ``Byzantium`` compatible
- New opcodes ``REVERT``, ``RETURNDATA`` and ``STATICCALL``
- Precompiles for curve operations and bigint mod exp
- Transaction return data in receipts
- For detailed list of changes see PR [#161](https://github.com/ethereumjs/ethereumjs-vm/pull/161) 
- For a ``Spurious Dragon``/``EIP 150`` compatible version of this library install latest version of ``2.2.x``

[2.3.1]: https://github.com/ethereumjs/ethereumjs-vm/compare/v2.2.2...v2.3.1

## [2.3.0] - Version Skipped due to faulty npm release

## [2.2.2] - 2017-09-19
- Fixed [JS number issues](https://github.com/ethereumjs/ethereumjs-vm/pull/168)
  and [certain edge cases](https://github.com/ethereumjs/ethereumjs-vm/pull/188)
- Fixed various smaller bugs and improved code consistency
- Some VM speedups
- Testing improvements
- Narrowed down dependencies for library not to break after Byzantium release

[2.2.2]: https://github.com/ethereumjs/ethereumjs-vm/compare/v2.2.1...v2.2.2

## [2.2.1] - 2017-08-04
- Fixed bug prevent the library to be used in the browser

[2.2.1]: https://github.com/ethereumjs/ethereumjs-vm/compare/v2.2.0...v2.2.1

## [2.2.0] - 2017-07-28
- ``Spurious Dragon`` & ``EIP 150`` compatible
- Detailed list of changes in pull requests [#147](https://github.com/ethereumjs/ethereumjs-vm/pull/147) and  [#143](https://github.com/ethereumjs/ethereumjs-vm/pull/143)
- Removed ``enableHomestead`` option when creating a [ new VM object](https://github.com/ethereumjs/ethereumjs-vm#new-vmstatetrie-blockchain) (pre-Homestead fork rules not supported any more)

[2.2.0]: https://github.com/ethereumjs/ethereumjs-vm/compare/v2.1.0...v2.2.0

## [2.1.0] - 2017-06-28
- Homestead compatible
- update state test runner for General State Tests

[2.1.0]: https://github.com/ethereumjs/ethereumjs-vm/compare/v2.0.1...v2.1.0

## Older releases:

- [2.0.1](https://github.com/ethereumjs/ethereumjs-vm/compare/v2.0.0...v2.0.1) - 2016-10-31
- [2.0.0](https://github.com/ethereumjs/ethereumjs-vm/compare/v1.4.0...v2.0.0) - 2016-09-26
- [1.4.0](https://github.com/ethereumjs/ethereumjs-vm/compare/v1.3.0...v1.4.0) - 2016-05-20
- [1.3.0](https://github.com/ethereumjs/ethereumjs-vm/compare/v1.2.2...v1.3.0) - 2016-04-02
- [1.2.2](https://github.com/ethereumjs/ethereumjs-vm/compare/v1.2.1...v1.2.2) - 2016-03-31
- [1.2.1](https://github.com/ethereumjs/ethereumjs-vm/compare/v1.2.0...v1.2.1) - 2016-03-03
- [1.2.0](https://github.com/ethereumjs/ethereumjs-vm/compare/v1.1.0...v1.2.0) - 2016-02-27
- [1.1.0](https://github.com/ethereumjs/ethereumjs-vm/compare/v1.0.4...v1.1.0) - 2016-01-09
- [1.0.4](https://github.com/ethereumjs/ethereumjs-vm/compare/v1.0.3...v1.0.4) - 2015-12-18
- [1.0.3](https://github.com/ethereumjs/ethereumjs-vm/compare/v1.0.0...v1.0.3) - 2015-11-27
- 1.0.0 - 2015-10-06
