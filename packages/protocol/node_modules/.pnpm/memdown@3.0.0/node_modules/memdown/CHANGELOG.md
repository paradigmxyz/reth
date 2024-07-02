# Changelog

## [Unreleased]

## [3.0.0] - 2018-05-22

### Added
* Add node 9 and 10 to Travis (@vweevers, @ralphtheninja)

### Changed
* Upgrade `abstract-leveldown` to `5.0.0` (@ralphtheninja)
* Upgrade `standard` to `11.0.0` (@ralphtheninja)
* Tweak readme (@ralphtheninja)
* Use `airtap` instead of `zuul` (@vweevers)
* Switch to plain MIT license (@vweevers)

### Removed
* Remove TypeScript typings (@ralphtheninja)
* Remove TypeScript tests (@vweevers)
* Remove node 4 from Travis (@ralphtheninja)
* Remove deprecated JWT addon from Travis (@vweevers)
* Remove obsolete `--stderr` flag (@vweevers)

## [2.0.0] - 2018-02-11

### Added
* Run test suite with TypeScript in addition to Node.js (@vweevers)
* Add `UPGRADING.md` (@vweevers)
* Add `CHANGELOG.md` (@vweevers)
* README: explain types and snapshot guarantees (@vweevers)
* README: add level badge (@ralphtheninja)
* README: add node version badge (@ralphtheninja)

### Changed
* Update `abstract-leveldown` to 4.0.0 (@vweevers)
* Perform serialization through idiomatic `_serializeKey` and `_serializeValue` (@vweevers)
* Don't stringify anything except nullish values (@vweevers)
* Use `Buffer.isBuffer()` instead of `AbstractLevelDOWN#isBuffer` (@vweevers)
* README: update instantiation instructions for latest `levelup` (@kumavis)
* README: rename "database" to "store" (@ralphtheninja)
* README: simplify example and prefer ES6 (@vweevers)
* Configure Greenkeeper to ignore updates to `@types/node` (@ralphtheninja)

### Fixed
* Don't clone `Buffer` in iterator (@vweevers)
* Stringify `Buffer.from()` argument in iterator (@vweevers)
* README: use SVG rather than PNG badge for Travis (@ralphtheninja)
* README: link to `abstract-leveldown` (@vweevers)
* README: normalize markdown headers (@ralphtheninja)
* README: fix license typos (@ralphtheninja)
* README: fix code example (@ralphtheninja)
* Rename `iterator#_end` to fix conflict with `abstract-leveldown` (@vweevers)
* Set `zuul --concurrency` to 1 to avoid hitting Sauce Labs limit (@vweevers)
* Test on Android 6.0 instead of latest (7.1) due to Sauce Labs issue (@vweevers)

### Removed
* Remove global store (@vweevers)
* Remove skipping of falsy elements in `MemDOWN#batch` (@vweevers)
* Remove obsolete benchmarks (@vweevers)
* Remove obsolete `testBuffer` from `test.js` (@vweevers)
* Remove redundant `testCommon` parameter from most tests (@vweevers)
* Remove unnecessary `rimraf` replacement for Browserify (@vweevers)
* README: remove Greenkeeper badge (@ralphtheninja)

[Unreleased]: https://github.com/level/memdown/compare/v3.0.0...HEAD
[3.0.0]: https://github.com/level/memdown/compare/v2.0.0...v3.0.0
[2.0.0]: https://github.com/level/memdown/compare/v1.4.1...v2.0.0
