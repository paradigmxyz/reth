# Changelog

## [Unreleased]

## [4.0.2] - 2018-05-30

### Changed
* Replace `util.inherits` with `inherits` module (@ralphtheninja)

## [4.0.1] - 2018-05-22

### Changed
* Upgrade `abstract-leveldown` to `5.0.0` (@ralphtheninja)

## [4.0.0] - 2018-05-13

### Added
* Add node 10 to Travis (@ralphtheninja)

### Changed
* Upgrade to `standard@11.0.0` (@ralphtheninja)

### Removed
* Remove node 4 from Travis (@ralphtheninja)

## [3.0.0] - 2018-02-08

### Added
* Add `9` to travis (@ralphtheninja)

### Changed
* Upgrade to `abstract-leveldown@4.0.0` (@ralphtheninja)

### Removed
* Remove  `DeferredLevelDOWN.prototype._isBuffer`, no longer needed since we use `Buffer.isBuffer()` (@ralphtheninja)

### Fixed
* Defer `approximateSize()` separately (@ralphtheninja)
* Fix broken link in `README` and clean up old `.jshintrc` (@ralphtheninja)

**Historical Note** `abstract-leveldown@4.0.0` dropped `approximateSize()` so we needed to defer this method separately for stores that support it.

## [2.0.3] - 2017-11-04

### Added
* Add `4` to travis (@vweevers)
* Add node badge (@vweevers)

### Changed
* Upgrade to `abstract-leveldown@3.0.0` (@vweevers)

**Historical Note** `abstract-leveldown@3.0.0` dropped support for node `0.12` and didn't have any breaking changes to api or behavior, hence a new patch version.

## [2.0.2] - 2017-10-06

### Added
* Add `standard` for linting (@ralphtheninja)

### Changed
* Use svg instead of png for travis badge (@ralphtheninja)
* Update to new badge setup (@ralphtheninja)

### Fixed
* `_serializeKey()` and `_serializeValue()` should not modify keys or values (@ralphtheninja)

## [2.0.1] - 2017-09-12

### Added
* Add Greenkeeper badge (@ralphtheninja)
* Add `6` and `8` to travis (@ralphtheninja)

### Changed
* Upgrade to `abstract-leveldown@2.7.0` (@ralphtheninja)

### Removed
* Remove `0.8`, `0.10` and `0.11` from travis (@ralphtheninja)

## [2.0.0] - 2017-07-30

### Changed
* Update dependencies (@ralphtheninja)
* Update copyright years (@ralphtheninja)

## [2.0.0-2] - 2015-05-28

### Fixed
* Fix `.iterator()` after db is opened (@juliangruber)

## [2.0.0-1] - 2015-05-28

No changes.

## [2.0.0-0] - 2015-05-27

### Changed
* Upgrade to `abstract-leveldown@2.4.0` for `.status` (@juliangruber)
* Change api to `leveldown` api (@juliangruber)

## [1.2.2] - 2017-07-30

### Added
* Add `4`, `6` and `7` to travis (@juliangruber)
* Add `8` to travis (@ralphtheninja)

### Changed
* Update `tape` and `abstract-leveldown` dependencies (@juliangruber)

### Removed
* Remove `0.10` from travis (@juliangruber)

## [1.2.1] - 2015-08-14

### Added
* Add `0.12`, `2.5` and `3.0` to travis (@juliangruber)

### Removed
* Remove `0.8` and `0.11` from travis (@juliangruber)

### Fixed
* Fix iterator after `setDb` case (@substack)
* Fix broken travis link (@juliangruber)

## [1.2.0] - 2015-05-28

### Changed
* Upgrade to `abstract-leveldown@2.4.0` for `.status` (@juliangruber)

## [1.1.0] - 2015-05-22

### Changed
* Export `DeferredIterator` (@juliangruber)

## [1.0.0] - 2015-04-28

### Changed
* Upgrade to `abstract-leveldown@2.1.2` (@ralphtheninja)

## [0.3.0] - 2015-04-16

### Added
* Add support for deferred iterators (@juliangruber)

### Changed
* Change to plain `MIT` license (@andrewrk)
* Update logo and copyright (@ralphtheninja)

## [0.2.0] - 2014-04-26

### Removed
* Remove `bops` and replace with `Buffer` (@rvagg)

## [0.1.0] - 2013-10-14

### Changed
* `location` passed to `AbstractLevelDOWN` constructor is optional (@rvagg)

### Removed
* Remove `npm-dl` badge (@rvagg)

### Fixed
* Fix broken travis badge (@rvagg)
* Fix links from `rvagg/` to `Level/` (@rvagg)

## [0.0.1] - 2013-09-30

### Added
* Add tests (@rvagg)
* Add node `0.10` and `0.11` to travis (@rvagg)

### Changed
* Update documentation (@rvagg)

## 0.0.0 - 2013-09-17

:seedling: First release. (@rvagg)

[Unreleased]: https://github.com/level/deferred-leveldown/compare/v4.0.2...HEAD
[4.0.2]: https://github.com/level/deferred-leveldown/compare/v4.0.1...v4.0.2
[4.0.1]: https://github.com/level/deferred-leveldown/compare/v4.0.0...v4.0.1
[4.0.0]: https://github.com/level/deferred-leveldown/compare/v3.0.0...v4.0.0
[3.0.0]: https://github.com/level/deferred-leveldown/compare/v2.0.3...v3.0.0
[2.0.3]: https://github.com/level/deferred-leveldown/compare/v2.0.2...v2.0.3
[2.0.2]: https://github.com/level/deferred-leveldown/compare/v2.0.1...v2.0.2
[2.0.1]: https://github.com/level/deferred-leveldown/compare/v2.0.0...v2.0.1
[2.0.0]: https://github.com/level/deferred-leveldown/compare/v2.0.0-2...v2.0.0
[2.0.0-2]: https://github.com/level/deferred-leveldown/compare/v2.0.0-1...v2.0.0-2
[2.0.0-1]: https://github.com/level/deferred-leveldown/compare/v2.0.0-0...v2.0.0-1
[2.0.0-0]: https://github.com/level/deferred-leveldown/compare/v1.1.0...v2.0.0-0
[1.2.2]: https://github.com/level/deferred-leveldown/compare/v1.2.1...v1.2.2
[1.2.1]: https://github.com/level/deferred-leveldown/compare/v1.2.0...v1.2.1
[1.2.0]: https://github.com/level/deferred-leveldown/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/level/deferred-leveldown/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/level/deferred-leveldown/compare/v0.3.0...v1.0.0
[0.3.0]: https://github.com/level/deferred-leveldown/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/level/deferred-leveldown/compare/0.1.0...v0.2.0
[0.1.0]: https://github.com/level/deferred-leveldown/compare/0.0.1...0.1.0
[0.0.1]: https://github.com/level/deferred-leveldown/compare/0.0.0...0.0.1
