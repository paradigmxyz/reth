# Changelog

## [Unreleased]

## [5.0.0] - 2018-05-22

### Added
* Add node 10 to Travis (@ralphtheninja)
* Add `airtap` for browser tests (@vweevers)

### Changed
* Update `sinon` to `^5.0.0` (@ralphtheninja)
* Tweak README (@ralphtheninja)
* Replace `const` with `var` to support IE10 (@vweevers)

### Removed
* Remove node 4, 5 and 7 from Travis (@ralphtheninja)
* Remove TypeScript tests (@vweevers)
* Remove TypeScript typings (@ralphtheninja)

## [4.0.3] - 2018-02-21

### Changed
* Update `ts-node` to `^5.0.0` (@zixia)
* Update `standard` to `^11.0.0` (@ralphtheninja)

### Fixed
* Remove invalid TypeScript from `Batch` (@Tapppi)
* Add JSDoc to incorrectly inferred TypeScript types (@Tapppi)

## [4.0.2] - 2018-02-09

### Fixed
* Fix `iterator#next` to return `this` (@vweevers)

## [4.0.1] - 2018-02-09

### Added
* Run test suite in TypeScript in addition to Node.js (@vweevers)
* Add TypeScript smoke test (@vweevers)
* Add TypeScript readme section with stability badge (@vweevers)

### Fixed
* Update TypeScript typings for v4 (@vweevers)
* Use ES6 classes in tests to please TypeScript (@vweevers)
* Define default methods on prototype to please TypeScript (@vweevers)

### Removed
* Remove obsolete parameters from tests (@vweevers)

**Historical Note** This was released as a patch because it only changed tests
and TypeScript typings (which are marked experimental and don't follow semver).

## [4.0.0] - 2018-01-20

### Added
* Add `standard` for linting (#150) (@ralphtheninja)
* Test that callbacks are called asynchronously (@vweevers)
* Test serialization extensibility (@vweevers)
* Add @vweevers to contributors (@ralphtheninja)
* Add upgrade guide in `UPGRADING.md` (@ralphtheninja)
* Add node 9 to Travis (@ralphtheninja)

### Changed
* Ignore empty range options in `AbstractLevelDOWN#_setupIteratorOptions` (@ralphtheninja)
* Make `testCommon.js` the default value for `testCommon` parameter (@ralphtheninja)
* Use `Buffer.isBuffer()` instead of `AbstractLevelDOWN#isBuffer` (@ralphtheninja)
* Cleanup iterator tests (#161) (@ralphtheninja)
* Pass test function as a parameter instead of setting local global (@ralphtheninja)
* Assert batch type is `'put'` or `'del'` (@vweevers)
* Assert batch array elements are objects (@vweevers)

### Fixed
* Ensure stores are closed properly (fixes problems on Windows) (@ralphtheninja)
* Call back errors on next tick to avoid `zalgo` (@vweevers)

### Removed
* Remove `isLevelDOWN` function and corresponding tests (@ralphtheninja)
* Remove `AbstractLevelDOWN#approximateSize` method and corresponding tests (@ralphtheninja)
* Remove `testBuffer` in `abstract/put-get-del-test.js` (@ralphtheninja)
* Remove object value test in `abstract/put-test.js` (@vweevers)
* Remove serialize buffer tests (@vweevers)
* Remove serialize object tests (@vweevers)
* Remove `BufferType` parameter in `abstract/put-get-del-test.js`, use `Buffer` (@ralphtheninja)

## [3.0.0] - 2017-11-04

### Added
* Add node version badge (@vweevers)

### Fixed
* Fix errors in `index.d.ts` (@sandersn)

### Removed
* Drop support for `0.12`. Cause for new major version! (@vweevers)

## [2.7.2] - 2017-10-11

### Changed
* Update `README` with new style (@ralphtheninja)

## [2.7.1] - 2017-09-30

### Changed
* Refactor typings as ES2015 module (@MeirionHughes)

## [2.7.0] - 2017-09-12

### Added
* Add `TypeScript` definitions in `index.d.ts` (@MeirionHughes)

## [2.6.3] - 2017-09-05

### Added
* Add `GreenKeeper` (@ralphtheninja)
* Test key/value serialization (@bigeasy)
* Test `undefined` value serializing to empty string (@ralphtheninja)

### Changed
* Update dependencies (@ralphtheninja)
* Convert nullish values to empty strings (@bigeasy)
* Use `t.equal(a, b)` instead of `t.ok(a === b)` (@bigeasy)
* Relax tests for serializing object in `abstract/chained-batch-test.js` (@ralphtheninja)

### Fixed
* Document `.status` property (@ralphtheninja)

## [2.6.2] - 2017-07-30

### Added
* Test serialization extensibility (@juliangruber)

### Changed
* Update dependencies and float `devDependencies` (@ralphtheninja)
* Update copyright years (@ralphtheninja)
* Update node versions on Travis (@ralphtheninja)

### Fixed
* Fix put test on object serialization (@juliangruber)

## [2.6.1] - 2016-09-12

### Fixed
* Fix `null` case in default value serializer (fixes problems in `2.6.0`) (@juliangruber)

## [2.6.0] - 2016-03-10

### Added
* Add `collectBatchOps` function to buffer `_put` and `_del` inputs in `abstract/chained-batch-test.js` (@deanlandolt)

### Changed
* Use proto delegation to patch methods on db (@deanlandolt)
* Allow serialization functions to return buffers (@deanlandolt)

### Removed
* Remove unnecessary initialization hackery in `abstract/chained-batch-test.js` (@deanlandolt)

**Historical Note** This release was a breaking change. See @juliangruber's [comment](https://github.com/Level/abstract-leveldown/pull/85#issuecomment-246980978) for more information.

## [2.5.0] - 2016-05-01

### Added
* Add dependency badge to `README` (@ralphtheninja)
* Add `AbstractLevelDOWN#_serializeKey` (@juliangruber)
* Add `AbstractLevelDOWN#_serializeValue` (@juliangruber)
* Add `AbstractChainedBatch#_serializeKey` (@juliangruber)
* Add `AbstractChainedBatch#_serializeValue` (@juliangruber)
* Test `_serialize` with object and buffer (@juliangruber)

### Changed
* Update dependencies and add more node versions to Travis (@ralphtheninja)

### Fixed
* Update `memdown` url (@ralphtheninja)
* `AbstractLevelDOWN#._checkKey` does not take three parameters (@ralphtheninja)
* Only show build status for the master branch (@watson)
* Fix minor typos in `README` (@timkuijsten)

### Removed
* Remove stringification of keys and values (@juliangruber)
* Remove `.toBuffer` (@juliangruber)

## [2.4.1] - 2015-08-29

### Fixed
* Remove use of `const` (@nolanlawson)

## [2.4.0] - 2015-05-19

### Added
* Add `.status` property to `AbstractLevelDOWN` (@juliangruber)

## [2.3.1] - 2015-05-18

### Added
* Link to `level/community` (@ralphtheninja)

### Fixed
* Document `isLevelDown` function (@ralphtheninja)

### Removed
* Extract `Contributors` section from `README` into `level/community` (@ralphtheninja)

## [2.3.0] - 2015-05-18

### Added
* Import `isLevelDOWN` function to `is-leveldown.js` (@ralphtheninja)

### Changed
* Use `t.equal(a, b)` instead of `t.ok(a === b)` (@juliangruber)
* Export API from `index.js` (@ralphtheninja)

## [2.2.2] - 2015-05-13

### Fixed
* Revert changes to `location` in `2.2.1` (@juliangruber)

## [2.2.1] - 2015-05-12

### Fixed
* Copy paste error gave wrong test description (@ralphtheninja)
* `t.throws()` is different for `tape` (@ralphtheninja)
* Assert `location` is not an empty string (@ralphtheninja)

## [2.2.0] - 2015-05-10

### Added
* Test `{ sync: true }` option in `abstract/put-test.js` (@juliangruber)

## [2.1.4] - 2015-04-28

### Fixed
* Use `t.equal()` with `tape` (@ralphtheninja)

## [2.1.3] - 2015-04-28

### Changed
* Change from `tap` to `tape` (@ralphtheninja)

## [2.1.2] - 2015-04-27

### Changed
* Convert buffer to string so we can compare (@ralphtheninja)

## [2.1.1] - 2015-04-27

### Added
* Add @ralphtheninja to contributors (@ralphtheninja)
* Add `0.12` and `iojs` to Travis (@ralphtheninja)

### Changed
* Update logo and copyright (@ralphtheninja)

### Fixed
* Include `.nonErrorValues()` test in `abstract/put-get-del-test.js` (@hden)
* `rvagg/node-abstract-leveldown` moved to `level/abstract-leveldown` (@ralphtheninja)
* Fix Travis for `0.8` (@ralphtheninja)

## [2.1.0] - 2014-11-09

### Added
* Add @watson to contributors (@rvagg)

### Changed
* Use `setTimeout` instead of `process.nextTick` (@bigeasy)

### Fixed
* Don't fail if no value is returned by `._get` (@watson)
* Use `error` test function when testing for errors (@watson)

## [2.0.3] - 2014-10-02

No change.

## [2.0.2] - 2014-10-02

### Added
* Test atomic batch operations (@calvinmetcalf)

## [2.0.1] - 2014-09-01

### Changed
* Set default values for options to `.open`, `.get`, `.put`, `.del` and `.batch` (@watson)
* Update pattern for setting default options for the iterator (@watson)
* Allow boolean options to be falsy/truthy (@watson)

### Removed
* Remove default options that are too `LevelDOWN` specific (@watson)

## [2.0.0] - 2014-08-26

### Changed
* Switch to allowing writes of empty values, `null`, `undefined`, `''`, `[]` and empty buffer (@juliangruber)
* Rename `AbstractLevelDOWN#_checkKeyValue` to `AbstractLevelDOWN#_checkKey` (@rvagg)

## [1.0.0] - 2014-08-24

### Added
* Test that an error is thrown when location isn't a string (@calvinmetcalf)
* Test opening and closing the store (@calvinmetcalf)
* Test iterator with `limit` set to `0` (@watson)
* Add more tests to `abstract/batch-test.js` (@calvinmetcalf)
* Set default values of iterator options (@watson)
* Account for batch options that are `null` (@calvinmetcalf)

### Changed
* Ensure `Boolean` iterator options are `Boolean` (@watson)

### Removed
* Remove options.start hackery (@rvagg)

## [0.12.4] - 2014-08-20

### Added
* Test that `simple-iterator` returns buffers (@kesla)
* Test implicit snapshots (@kesla)

### Changed
* Change license to plain MIT (@andrewrk)

## [0.12.3] - 2014-06-27

### Changed
* Update `xtend` dependency (@andrewrk)

## [0.12.2] - 2014-04-26

### Changed
* Have `isTypedArray` check for existence of `ArrayBuffer` and `Uint8Array` constructors before usage (@rvagg)

## [0.12.1] - 2014-04-26

### Changed
* Set default `BufferType` in `abstract/put-get-del-test.js` to `Buffer` instead of `ArrayBuffer` (@maxogden)

## [0.12.0] - 2014-03-12

### Changed
* Revert to pure `Buffer` and remove usage of `Uint16Array` (@rvagg)

## [0.11.4] - 2014-03-11

### Removed
* Remove duplicate call to `t.end()` (@maxogden)

## [0.11.3] - 2014-01-26

### Changed
* Loosen the buffer type check (@rvagg)

## [0.11.2] - 2013-12-05

### Added
* Add npm badges (@rvagg)

### Fixed
* Fix iterator tests in `test.js` (@rvagg)

## [0.11.1] - 2013-11-15

### Changed
* Adjust `abstract/approximate-size-test.js` to account for snappy compression (@rvagg)

## [0.11.0] - 2013-10-14

### Added
* Normalize `iterator()` options with `AbstractLevelDOWN#_setupIteratorOptions` (@rvagg)

## [0.10.2] - 2013-09-06

### Changed
* Refactor duplicated versions of `isTypedArray` into `abstract/util.js` (@rvagg)
* Refactor duplicated versions of `'NotFound'` checks into `abstract/util.js`, fixed too-strict version in `get-test.js` (@rvagg)

## [0.10.1] - 2013-08-29

### Added
* Add @substack to contributors (@rvagg)

### Changed
* Relax check for `Not Found` error message to be case insensitive in `get-test.js` (@rvagg)

## [0.10.0] - 2013-08-19

### Added
* Test `gt`, `gte`, `lt` and `lte` ranges (@dominictarr)

## [0.9.0] - 2013-08-11

### Added
* Test simultaneous get's (@kesla)
* Test `AbstractChainedBatch` extensibility (@kesla)

### Changed
* Make `AbstractChainedBatch` extensible (@kesla)
* Export `AbstractChainedBatch` from `abstract-leveldown.js` (@kesla)

### Fixed
* Fix broken test assertion in `abstract/get-test.js` (@rvagg)
* Fix tests that weren't running properly (@kesla)

## [0.8.2] - 2013-08-02

No changes. Merely published changes made in `0.8.1`.

## [0.8.1] - 2013-08-02

### Changed
* Remove use of `const` in `testCommon.js` (@rvagg)

**Historical Note** The version in `package.json` was changed from `0.7.4` to `0.8.1`. The `0.8.1` tag exists but this version was never published to npm.

## [0.8.0] - 2013-08-02

### Added
* Add `BufferType` parameter to `abstract/put-get-del-test.js` for `bops` support (@rvagg)
* Add `isTypedArray` function which checks `ArrayBuffer` or `Uint8Array` for `bops` support (@rvagg)

### Changed
* Use `process.browser` check instead of `process.title == 'browser'` (@rvagg)

### Fixed
* Fix `cleanup` function not calling back when browserified (@rvagg)

**Historical Note** It seems the version in `package.json` was never changed to `0.8.0` in the git history, even though the `0.8.0` tag exists. Most likely `package.json` was modified locally during `npm publish` but was never committed.

## [0.7.4] - 2013-08-02

### Fixed
* Fix problems related to `browserify` and `rimraf` (@rvagg)

## [0.7.3] - 2013-07-26

### Added
* Add @pgte to contributors (@rvagg)
* Test iterator with `limit` set to `-1` (@kesla)

## [0.7.2] - 2013-07-08

### Added
* Add `AbstractChainedBatch#_checkWritten` (@rvagg)
* Test delete on non-existent key (@rvagg)
* Test iterator with `start` after database `end` (@juliangruber)

### Changed
* Freeze chained batch state after `.write()` has been called (@rvagg)
* Make `NotFound` error case insensitive (@rvagg)
* Use `self` rather than binding functions to `this` (@juliangruber)

### Fixed
* Don't coerce values to strings in browser (@maxogden)
* Make tests work in node and browser (@maxogden)

## [0.7.1] - 2013-05-15

### Changed
* Adjust tests to be browserable (@rvagg)

## [0.7.0] - 2013-05-14

### Added
* Add `AbstractChainedBatch#clear` (@rvagg)

## [0.6.1] - 2013-05-14

### Changed
* Make `AbstractIterator` call back with an error instead of throwing on nexting and ending (@mcollina)

## [0.6.0] - 2013-05-14

### Changed
* Split `t.deepEqual()` into multiple `t.equal()` in `abstract/iterator-test.js` (@rvagg)
* Make `AbstractIterator` call back with an error instead of throwing on nexting and ending (@mcollina)

## [0.5.0] - 2013-05-14

### Changed
* Make `iterator.end(cb)` and `iterator.next(cb)` call back with an error instead of throwing (@mcollina)

## [0.4.0-1] - 2013-05-14

### Added
* Add @No9 and @mcollina to contributors (@rvagg)

## [0.4.0] - 2013-05-14

### Added
* Add `AbstractChainedBatch` (@rvagg)
* Add `AbstractLevelDOWN#_chainedBatch` (@rvagg)
* Add `abstract/batch-test.js` and `abstract/chained-batch-test.js` (@rvagg)

### Changed
* Move `AbstractIterator` from `abstract-leveldown.js` to `abstract-iterator.js` (@rvagg)

## [0.3.0] - 2013-05-04

### Added
* Restore test for opening the database without options (@rvagg)
* Add `AbstractLevelDOWN#_isBuffer` so it can be overridden (@rvagg)
* Add `AbstractLevelDOWN#_checkKeyValue` so it can be overridden (@rvagg)

### Changed
* Use `this._checkKeyValue()` instead of local function (@rvagg)
* Use `this._isBuffer()` instead of `Buffer.isBuffer()` (@rvagg)

## [0.2.3] - 2013-05-04

### Removed
* Remove test for opening the database without options (@rvagg)

## [0.2.2] - 2013-05-04

### Changed
* Split `.open()` tests into `.open()` and `.openAdvanced()` (@rvagg)

## [0.2.1] - 2013-05-04

### Changed
* Convert values to `string` in `abstract/put-get-del-test.js` if `Buffer` is `undefined` (@rvagg)

## [0.2.0] - 2013-05-04

### Added
* Add `process.browser` check for `start` and `end` keys in browser (@maxogden)
* Add `levelup` contributors (@rvagg)

### Changed
* Convert values to `string` in `abstract/get-test.js` if `Buffer` is `undefined` (@rvagg)
* Don't stringify keys and values in `abstract/iterator-test.js` (@maxogden)

### Fixed
* Fix `tape` compatibility issues (@maxogden)

## [0.1.0] - 2013-04-23

### Added
* Import abstract tests from `leveldown` (@maxogden)

### Fixed
* Clarify `README` (@rvagg)

## [0.0.2] - 2013-03-18

### Added
* Add node 0.10 to Travis (@rvagg)
* Add `Buffer.isBuffer()` checks to keys and values (@rvagg)

### Changed
* Export `checkKeyValue` (@rvagg)

## [0.0.1] - 2013-03-18

### Added
* Add `checkKeyValue` function for more complete error checking (@rvagg)

## 0.0.0 - 2013-03-15

First release. :seedling:

[Unreleased]: https://github.com/level/abstract-leveldown/compare/v5.0.0...HEAD
[5.0.0]: https://github.com/level/abstract-leveldown/compare/v4.0.3...v5.0.0
[4.0.3]: https://github.com/level/abstract-leveldown/compare/v4.0.2...v4.0.3
[4.0.2]: https://github.com/level/abstract-leveldown/compare/v4.0.1...v4.0.2
[4.0.1]: https://github.com/level/abstract-leveldown/compare/v4.0.0...v4.0.1
[4.0.0]: https://github.com/level/abstract-leveldown/compare/v3.0.0...v4.0.0
[3.0.0]: https://github.com/level/abstract-leveldown/compare/v2.7.2...v3.0.0
[2.7.2]: https://github.com/level/abstract-leveldown/compare/v2.7.1...v2.7.2
[2.7.1]: https://github.com/level/abstract-leveldown/compare/v2.7.0...v2.7.1
[2.7.0]: https://github.com/level/abstract-leveldown/compare/v2.6.3...v2.7.0
[2.6.3]: https://github.com/level/abstract-leveldown/compare/v2.6.2...v2.6.3
[2.6.2]: https://github.com/level/abstract-leveldown/compare/v2.6.1...v2.6.2
[2.6.1]: https://github.com/level/abstract-leveldown/compare/v2.6.0...v2.6.1
[2.6.0]: https://github.com/level/abstract-leveldown/compare/v2.5.0...v2.6.0
[2.5.0]: https://github.com/level/abstract-leveldown/compare/v2.4.1...v2.5.0
[2.4.1]: https://github.com/level/abstract-leveldown/compare/v2.4.0...v2.4.1
[2.4.0]: https://github.com/level/abstract-leveldown/compare/v2.3.1...v2.4.0
[2.3.1]: https://github.com/level/abstract-leveldown/compare/v2.3.0...v2.3.1
[2.3.0]: https://github.com/level/abstract-leveldown/compare/v2.2.2...v2.3.0
[2.2.2]: https://github.com/level/abstract-leveldown/compare/v2.2.1...v2.2.2
[2.2.1]: https://github.com/level/abstract-leveldown/compare/v2.2.0...v2.2.1
[2.2.0]: https://github.com/level/abstract-leveldown/compare/v2.1.4...v2.2.0
[2.1.4]: https://github.com/level/abstract-leveldown/compare/v2.1.3...v2.1.4
[2.1.3]: https://github.com/level/abstract-leveldown/compare/v2.1.2...v2.1.3
[2.1.2]: https://github.com/level/abstract-leveldown/compare/v2.1.1...v2.1.2
[2.1.1]: https://github.com/level/abstract-leveldown/compare/v2.1.0...v2.1.1
[2.1.0]: https://github.com/level/abstract-leveldown/compare/v2.0.3...v2.1.0
[2.0.3]: https://github.com/level/abstract-leveldown/compare/v2.0.2...v2.0.3
[2.0.2]: https://github.com/level/abstract-leveldown/compare/v2.0.1...v2.0.2
[2.0.1]: https://github.com/level/abstract-leveldown/compare/v2.0.0...v2.0.1
[2.0.0]: https://github.com/level/abstract-leveldown/compare/v1.0.0...v2.0.0
[1.0.0]: https://github.com/level/abstract-leveldown/compare/v0.12.4...v1.0.0
[0.12.4]: https://github.com/level/abstract-leveldown/compare/v0.12.3...v0.12.4
[0.12.3]: https://github.com/level/abstract-leveldown/compare/v0.12.2...v0.12.3
[0.12.2]: https://github.com/level/abstract-leveldown/compare/v0.12.1...v0.12.2
[0.12.1]: https://github.com/level/abstract-leveldown/compare/v0.12.0...v0.12.1
[0.12.0]: https://github.com/level/abstract-leveldown/compare/v0.11.4...v0.12.0
[0.11.4]: https://github.com/level/abstract-leveldown/compare/v0.11.3...v0.11.4
[0.11.3]: https://github.com/level/abstract-leveldown/compare/v0.11.2...v0.11.3
[0.11.2]: https://github.com/level/abstract-leveldown/compare/0.11.1...v0.11.2
[0.11.1]: https://github.com/level/abstract-leveldown/compare/0.11.0...0.11.1
[0.11.0]: https://github.com/level/abstract-leveldown/compare/0.10.2...0.11.0
[0.10.2]: https://github.com/level/abstract-leveldown/compare/0.10.1...0.10.2
[0.10.1]: https://github.com/level/abstract-leveldown/compare/0.10.0...0.10.1
[0.10.0]: https://github.com/level/abstract-leveldown/compare/0.9.0...0.10.0
[0.9.0]: https://github.com/level/abstract-leveldown/compare/0.8.2...0.9.0
[0.8.2]: https://github.com/level/abstract-leveldown/compare/0.8.1...0.8.2
[0.8.1]: https://github.com/level/abstract-leveldown/compare/0.8.0...0.8.1
[0.8.0]: https://github.com/level/abstract-leveldown/compare/0.7.4...0.8.0
[0.7.4]: https://github.com/level/abstract-leveldown/compare/0.7.3...0.7.4
[0.7.3]: https://github.com/level/abstract-leveldown/compare/0.7.2...0.7.3
[0.7.2]: https://github.com/level/abstract-leveldown/compare/0.7.1...0.7.2
[0.7.1]: https://github.com/level/abstract-leveldown/compare/0.7.0...0.7.1
[0.7.0]: https://github.com/level/abstract-leveldown/compare/0.6.1...0.7.0
[0.6.1]: https://github.com/level/abstract-leveldown/compare/0.6.0...0.6.1
[0.6.0]: https://github.com/level/abstract-leveldown/compare/0.5.0...0.6.0
[0.5.0]: https://github.com/level/abstract-leveldown/compare/0.4.0-1...0.5.0
[0.4.0-1]: https://github.com/level/abstract-leveldown/compare/0.4.0...0.4.0-1
[0.4.0]: https://github.com/level/abstract-leveldown/compare/0.3.0...0.4.0
[0.3.0]: https://github.com/level/abstract-leveldown/compare/0.2.1...0.3.0
[0.2.3]: https://github.com/level/abstract-leveldown/compare/0.2.2...0.2.3
[0.2.2]: https://github.com/level/abstract-leveldown/compare/0.2.1...0.2.2
[0.2.1]: https://github.com/level/abstract-leveldown/compare/0.2.0...0.2.1
[0.2.0]: https://github.com/level/abstract-leveldown/compare/0.1.0...0.2.0
[0.1.0]: https://github.com/level/abstract-leveldown/compare/0.0.2...0.1.0
[0.0.2]: https://github.com/level/abstract-leveldown/compare/0.0.1...0.0.2
[0.0.1]: https://github.com/level/abstract-leveldown/compare/0.0.0...0.0.1
