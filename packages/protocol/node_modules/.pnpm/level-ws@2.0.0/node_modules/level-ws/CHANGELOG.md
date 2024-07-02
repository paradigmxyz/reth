# Changelog

_**If you are upgrading:** please see [`UPGRADING.md`](UPGRADING.md)._

## [Unreleased][unreleased]

## [2.0.0] - 2019-03-30

### Changed

- Upgrade `readable-stream` from `^2.2.8` to `^3.1.0` ([#105](https://github.com/Level/level-ws/issues/105)) ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Upgrade `level` devDependency from `^4.0.0` to `^5.0.1` ([#109](https://github.com/Level/level-ws/issues/109)) ([**@vweevers**](https://github.com/vweevers))
- Upgrade `nyc` devDependency from `^12.0.2` to `^13.2.0` ([#108](https://github.com/Level/level-ws/issues/108)) ([**@vweevers**](https://github.com/vweevers))
- Upgrade `standard` devDependency from `^11.0.1` to `^12.0.0` ([#102](https://github.com/Level/level-ws/issues/102)) ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Apply common project tweaks ([#106](https://github.com/Level/level-ws/issues/106), [#107](https://github.com/Level/level-ws/issues/107)) ([**@vweevers**](https://github.com/vweevers))

### Removed

- Remove node 9 ([`6e1ef3b`](https://github.com/Level/level-ws/commit/6e1ef3b)) ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [1.0.0] - 2018-06-30

### Changed

- Refactor test options to always set `createIfMissing` and `errorIfExists` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Move `setUp()` function into `test()` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Move `openTestDatabase()` calls into `test()` and pass `ctx` to tests ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Test error after `db.close()` and after `cleanup()` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Use `after` in `cleanup()` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Use only `readable-stream` from user land ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Use `^` for devDependencies ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Switch to plain MIT license ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Replace `util.inherits` with `inherits` module ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Replace `this._destroyed` with `this.destroyed` from `Writable` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Export single function that creates the stream ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Flip parameters in `WriteStream` constructor ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Verify results once using `level-concat-iterator` intead of multiple `db.get()` operations ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Update README style ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Optimize internal batch `_buffer` by pushing transformed data ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Use `tempy` for test locations and remove `cleanup()` ([**@vweevers**](https://github.com/vweevers))
- Pass complete object in `_write()` extending default type ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Link to node 8 lts version of `Writable` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Support custom `highWaterMark` ([**@vweevers**](https://github.com/vweevers))
- Change `maxBufferLength` to pause rather than drop writes ([**@vweevers**](https://github.com/vweevers))

### Added

- Add node 6, 8, 9 and 10 to Travis ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Add `standard` for linting ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Test race condition ([**@vweevers**](https://github.com/vweevers))
- Add `nyc` and `coveralls` ([**@vweevers**](https://github.com/vweevers))
- Add `CHANGELOG.md` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Add `UPGRADING.md` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Test `maxBufferLength` ([**@vweevers**](https://github.com/vweevers))
- Test edge cases ([**@vweevers**](https://github.com/vweevers))

### Removed

- Remove node 0.10, 2, 3, 4 and 5 from Travis ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Remove `contributors` from `package.json` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Remove copyright headers from code ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Remove `this.{writable,readable}` state ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Remove `this._db.isOpen()` checks ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Remove patching db from the API ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Remove default `'utf8'` encoding and per stream encodings ([**@ralphtheninja**](https://github.com/ralphtheninja), [**@vweevers**](https://github.com/vweevers))
- Remove `.jshintrc` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Remove `WriteStream#destroySoon()` ([**@vweevers**](https://github.com/vweevers))
- Remove `WriteStream#toString()` ([**@vweevers**](https://github.com/vweevers))
- Remove redundant `!buffer` check ([**@vweevers**](https://github.com/vweevers))

### Fixed

- Fix erroneous test on missing type ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Fix race condition by flushing before finish ([**@vweevers**](https://github.com/vweevers))
- Fix `_destroy` to emit `'close'` after error ([**@vweevers**](https://github.com/vweevers))

## [0.1.0] - 2017-04-07

### Changed

- Upgrade `readable-stream` from `~2.0.6` to `^2.2.8` ([**@mcollina**](https://github.com/mcollina))
- Upgrade `xtend` from `~2.2.1` to `^4.0.0` ([**@mcollina**](https://github.com/mcollina))

## [0.0.1] - 2016-03-14

### Changed

- Upgrade `readable-stream` from `~1.0.15` to `~2.0.6` ([**@rvagg**](https://github.com/rvagg), [**@greenkeeper**](https://github.com/greenkeeper))
- Use `__dirname` instead of temporary directory ([**@rvagg**](https://github.com/rvagg))
- Update logo and copyright ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Added

- Add Travis ([**@rvagg**](https://github.com/rvagg))

## 0.0.0 - 2013-10-12

:seedling: Initial release.

[unreleased]: https://github.com/level/level-ws/compare/v2.0.0...HEAD

[2.0.0]: https://github.com/level/level-ws/compare/v1.0.0...v2.0.0

[1.0.0]: https://github.com/level/level-ws/compare/v0.1.0...v1.0.0

[0.1.0]: https://github.com/level/level-ws/compare/v0.0.1...v0.1.0

[0.0.1]: https://github.com/level/level-ws/compare/v0.0.0...v0.0.1
