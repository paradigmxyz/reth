# Changelog

_**If you are upgrading:** please see [`UPGRADING.md`](https://github.com/Level/memdown/blob/master/UPGRADING.md)._

## [5.1.0] - 2019-10-04

### Changed

- Upgrade `hallmark` devDependency from `^1.0.0` to `^2.0.0` ([#196](https://github.com/Level/memdown/issues/196)) ([**@vweevers**](https://github.com/vweevers))
- Upgrade `standard` devDependency from `^13.0.1` to `^14.0.0` ([#195](https://github.com/Level/memdown/issues/195)) ([**@vweevers**](https://github.com/vweevers))

### Added

- Add manifest ([Level/community#83](https://github.com/Level/community/issues/83)) ([#198](https://github.com/Level/memdown/issues/198)) ([**@vweevers**](https://github.com/vweevers))

## [5.0.0] - 2019-08-18

### Changed

- Upgrade `abstract-leveldown` from `~6.0.1` to `~6.1.0` ([#194](https://github.com/Level/memdown/issues/194)) ([**@vweevers**](https://github.com/vweevers))
- Upgrade `safe-buffer` from `~5.1.1` to `~5.2.0` ([#187](https://github.com/Level/memdown/issues/187)) ([**@vweevers**](https://github.com/vweevers))
- Upgrade `hallmark` devDependency from `^0.1.0` to `^1.0.0` ([#189](https://github.com/Level/memdown/issues/189)) ([**@vweevers**](https://github.com/vweevers))
- Upgrade `standard` devDependency from `^12.0.0` to `^13.0.1` ([#188](https://github.com/Level/memdown/issues/188)) ([**@vweevers**](https://github.com/vweevers))

### Added

- Opt-in to new [`clear()`](https://github.com/Level/abstract-leveldown#dbclearoptions-callback) tests ([#194](https://github.com/Level/memdown/issues/194)) ([**@vweevers**](https://github.com/vweevers))

### Removed

- Drop support of key & value types other than string and Buffer ([#191](https://github.com/Level/memdown/issues/191), [#192](https://github.com/Level/memdown/issues/192)) ([**@vweevers**](https://github.com/vweevers))

## [4.1.0] - 2019-06-28

### Changed

- Upgrade `nyc` devDependency from `^13.2.0` to `^14.0.0` ([#182](https://github.com/Level/memdown/issues/182)) ([**@vweevers**](https://github.com/vweevers))

### Added

- Support seeking ([#184](https://github.com/Level/memdown/issues/184)) ([**@MeirionHughes**](https://github.com/MeirionHughes))

## [4.0.0] - 2019-03-29

### Changed

- Upgrade `abstract-leveldown` from `~5.0.0` to `~6.0.1` ([#174](https://github.com/Level/memdown/issues/174)) ([**@vweevers**](https://github.com/vweevers))
- Invoke abstract tests from single function ([#174](https://github.com/Level/memdown/issues/174)) ([**@vweevers**](https://github.com/vweevers))
- Use `level-concat-iterator` and `testCommon.factory()` in custom tests ([#174](https://github.com/Level/memdown/issues/174)) ([**@vweevers**](https://github.com/vweevers))
- Don't use falsy or undefined as not-defined signal ([#174](https://github.com/Level/memdown/issues/174)) ([**@vweevers**](https://github.com/vweevers))
  ([**@vweevers**](https://github.com/vweevers))
- Upgrade `standard` devDependency from `^11.0.0` to `^12.0.0` ([#173](https://github.com/Level/memdown/issues/173)) ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Upgrade `airtap` devDependency from `^0.0.5` to `^2.0.0` ([#161](https://github.com/Level/memdown/issues/161), [#163](https://github.com/Level/memdown/issues/163), [#168](https://github.com/Level/memdown/issues/168), [#172](https://github.com/Level/memdown/issues/172), [#177](https://github.com/Level/memdown/issues/177)) ([**@vweevers**](https://github.com/vweevers))
- Tweak copyright years for less maintenance ([`760b375`](https://github.com/Level/memdown/commit/760b375)) ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Apply common project tweaks ([#178](https://github.com/Level/memdown/issues/178), [#179](https://github.com/Level/memdown/issues/179)) ([**@vweevers**](https://github.com/vweevers))

### Added

- Add `nyc` and browser code coverage ([#169](https://github.com/Level/memdown/issues/169), [#180](https://github.com/Level/memdown/issues/180)) ([**@vweevers**](https://github.com/vweevers))
- Add Sauce Labs logo to README ([#165](https://github.com/Level/memdown/issues/165)) ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Document that nullish values are now also rejected ([#174](https://github.com/Level/memdown/issues/174)) ([**@vweevers**](https://github.com/vweevers))
- Test negative and positive `Infinity` as keys ([#174](https://github.com/Level/memdown/issues/174)) ([**@vweevers**](https://github.com/vweevers))

### Removed

- Remove contributors from `package.json` ([`80b3e3a`](https://github.com/Level/memdown/commit/80b3e3a)) ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Remove node 9 ([`0de8721`](https://github.com/Level/memdown/commit/0de8721)) ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Remove now irrelevant serialization of nullish values ([#174](https://github.com/Level/memdown/issues/174)) ([**@vweevers**](https://github.com/vweevers))
- Remove dummy location from `abstract-leveldown` constructor call ([#174](https://github.com/Level/memdown/issues/174)) ([**@vweevers**](https://github.com/vweevers))
- Remove `rimraf` devDependency ([#174](https://github.com/Level/memdown/issues/174)) ([**@vweevers**](https://github.com/vweevers))

### Fixed

- Fix link references in `UPGRADING.md` for latest `remark` ([`f111a6f`](https://github.com/Level/memdown/commit/f111a6f)) ([**@vweevers**](https://github.com/vweevers))

## [3.0.0] - 2018-05-22

### Added

- Add node 9 and 10 to Travis ([**@vweevers**](https://github.com/vweevers), [**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

- Upgrade `abstract-leveldown` to `5.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Upgrade `standard` to `11.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Tweak readme ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Use `airtap` instead of `zuul` ([**@vweevers**](https://github.com/vweevers))
- Switch to plain MIT license ([**@vweevers**](https://github.com/vweevers))

### Removed

- Remove TypeScript typings ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Remove TypeScript tests ([**@vweevers**](https://github.com/vweevers))
- Remove node 4 from Travis ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Remove deprecated JWT addon from Travis ([**@vweevers**](https://github.com/vweevers))
- Remove obsolete `--stderr` flag ([**@vweevers**](https://github.com/vweevers))

## [2.0.0] - 2018-02-11

### Added

- Run test suite with TypeScript in addition to Node.js ([**@vweevers**](https://github.com/vweevers))
- Add `UPGRADING.md` ([**@vweevers**](https://github.com/vweevers))
- Add `CHANGELOG.md` ([**@vweevers**](https://github.com/vweevers))
- README: explain types and snapshot guarantees ([**@vweevers**](https://github.com/vweevers))
- README: add level badge ([**@ralphtheninja**](https://github.com/ralphtheninja))
- README: add node version badge ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

- Update `abstract-leveldown` to 4.0.0 ([**@vweevers**](https://github.com/vweevers))
- Perform serialization through idiomatic `_serializeKey` and `_serializeValue` ([**@vweevers**](https://github.com/vweevers))
- Don't stringify anything except nullish values ([**@vweevers**](https://github.com/vweevers))
- Use `Buffer.isBuffer()` instead of `AbstractLevelDOWN#isBuffer` ([**@vweevers**](https://github.com/vweevers))
- README: update instantiation instructions for latest `levelup` ([**@kumavis**](https://github.com/kumavis))
- README: rename "database" to "store" ([**@ralphtheninja**](https://github.com/ralphtheninja))
- README: simplify example and prefer ES6 ([**@vweevers**](https://github.com/vweevers))
- Configure Greenkeeper to ignore updates to `@types/node` ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Fixed

- Don't clone `Buffer` in iterator ([**@vweevers**](https://github.com/vweevers))
- Stringify `Buffer.from()` argument in iterator ([**@vweevers**](https://github.com/vweevers))
- README: use SVG rather than PNG badge for Travis ([**@ralphtheninja**](https://github.com/ralphtheninja))
- README: link to `abstract-leveldown` ([**@vweevers**](https://github.com/vweevers))
- README: normalize markdown headers ([**@ralphtheninja**](https://github.com/ralphtheninja))
- README: fix license typos ([**@ralphtheninja**](https://github.com/ralphtheninja))
- README: fix code example ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Rename `iterator#_end` to fix conflict with `abstract-leveldown` ([**@vweevers**](https://github.com/vweevers))
- Set `zuul --concurrency` to 1 to avoid hitting Sauce Labs limit ([**@vweevers**](https://github.com/vweevers))
- Test on Android 6.0 instead of latest (7.1) due to Sauce Labs issue ([**@vweevers**](https://github.com/vweevers))

### Removed

- Remove global store ([**@vweevers**](https://github.com/vweevers))
- Remove skipping of falsy elements in `MemDOWN#batch` ([**@vweevers**](https://github.com/vweevers))
- Remove obsolete benchmarks ([**@vweevers**](https://github.com/vweevers))
- Remove obsolete `testBuffer` from `test.js` ([**@vweevers**](https://github.com/vweevers))
- Remove redundant `testCommon` parameter from most tests ([**@vweevers**](https://github.com/vweevers))
- Remove unnecessary `rimraf` replacement for Browserify ([**@vweevers**](https://github.com/vweevers))
- README: remove Greenkeeper badge ([**@ralphtheninja**](https://github.com/ralphtheninja))

[5.1.0]: https://github.com/Level/memdown/compare/v5.0.0...v5.1.0

[5.0.0]: https://github.com/Level/memdown/compare/v4.1.0...v5.0.0

[4.1.0]: https://github.com/Level/memdown/compare/v4.0.0...v4.1.0

[4.0.0]: https://github.com/Level/memdown/compare/v3.0.0...v4.0.0

[3.0.0]: https://github.com/Level/memdown/compare/v2.0.0...v3.0.0

[2.0.0]: https://github.com/level/memdown/compare/v1.4.1...v2.0.0
