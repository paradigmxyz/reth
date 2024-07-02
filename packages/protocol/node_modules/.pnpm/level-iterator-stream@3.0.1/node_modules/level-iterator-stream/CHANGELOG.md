# Changelog

## [Unreleased]

## [3.0.1] - 2018-10-18

### Fixed
* Bump `readable-stream` from `^2.0` to `^2.3` to prevent npm dedupe and ensure it has `#destroy()` (@vweevers)

## [3.0.0] - 2018-06-28

### Removed
* Remove node 4 (@ralphtheninja)

## [2.0.2] - 2018-06-28

### Changed
* Restore node 4 (@ralphtheninja)

**Historical Note** We made a mistake releasing `v2.0.1` with the `engines` field in `package.json` set to node 6 as minimal version. This caused problems for users of yarn. We therefore released `v2.0.2` which restored node 4 and a new major directly after this.

## [2.0.1] - 2018-06-10

### Changed
* Upgrade `leveldown` devDependency from `^1.4.1` to `^4.0.0` (@ralphtheninja)
* Upgrade `standard` devDependency from `^10.0.3` to `^11.0.0` (@ralphtheninja)

### Added
* Add node 9 and 10 to Travis (@ralphtheninja)
* Add `UPGRADING.md` (@ralphtheninja)

### Removed
* Remove node 7 from Travis (@ralphtheninja)

## [2.0.0] - 2017-08-28

### Changed
* Upgrade `readable-stream` from `^1.0.33` to `^2.0.5` (@greenkeeper, @ralphtheninja)
* Upgrade `tape` devDependency from `^3.5.0` to `^4.4.0` (@greenkeeper, @ralphtheninja)
* Upgrade `through2` devDependency from `^0.6.3` to `^2.0.0` (@greenkeeper, @ralphtheninja)
* Upgrade `leveldown` devDependency from `^0.10.4` to `^1.4.1` (@juliangruber)
* Update copyright year to 2017 (@ralphtheninja)
* Update README example using `standard` (@ralphtheninja)

### Added
* Add node 6 to Travis (@greenkeeper, @juliangruber)
* Add node 7 and 8 to Travis (@ralphtheninja)
* Add Greenkeeper (@ralphtheninja)
* Add `standard` (@ralphtheninja)
* Test `.destroy()` during and after `iterator.next()` (@ralphtheninja)

### Removed
* Remove node 0.10, 0.12 and iojs from Travis (@greenkeeper, @juliangruber)
* Remove encodings (@ralphtheninja)
* Remove Makefile (@ralphtheninja)

## [1.3.1] - 2015-08-17

### Changed
* Update `.repository` path in `package.json` (@timoxley)

### Fixed
* Use `level-codec` from npm (@juliangruber)

## [1.3.0] - 2015-05-05

### Fixed
* Emit `'close'` after `'error'` (@juliangruber)

## [1.2.0] - 2015-05-04

### Added
* Add `.decoder` option to constructor for decoding keys and values (@juliangruber)

## [1.1.1] - 2015-03-29

### Added
* Enable Travis and add node 0.10, 0.12 and iojs (@ralphtheninja)
* Add MIT license (@ralphtheninja)

### Fixed
* Fix race condition in `.destroy()` (@juliangruber)

## [1.1.0] - 2015-03-29

### Added
* Add `.destroy()` (@juliangruber)

## 1.0.0 - 2015-03-29

:seedling: Initial release.

[Unreleased]: https://github.com/level/iterator-stream/compare/v3.0.1...HEAD
[3.0.1]: https://github.com/level/iterator-stream/compare/v3.0.0...v3.0.1
[3.0.0]: https://github.com/level/iterator-stream/compare/v2.0.2...v3.0.0
[2.0.2]: https://github.com/level/iterator-stream/compare/v2.0.1...v2.0.2
[2.0.1]: https://github.com/level/iterator-stream/compare/v2.0.0...v2.0.1
[2.0.0]: https://github.com/level/iterator-stream/compare/v1.3.1...v2.0.0
[1.3.1]: https://github.com/level/iterator-stream/compare/v1.3.0...v1.3.1
[1.3.0]: https://github.com/level/iterator-stream/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/level/iterator-stream/compare/v1.1.1...v1.2.0
[1.1.1]: https://github.com/level/iterator-stream/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/level/iterator-stream/compare/v1.0.0...v1.1.0
