# Changelog

## [Unreleased]

## [5.0.4] - 2018-06-22

### Added

-   Add `LICENSE.md` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Add `CONTRIBUTORS.md` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Add `remark` tooling ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [5.0.3] - 2018-05-30

### Changed

-   Replace `util.inherits` with `inherits` module ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [5.0.2] - 2018-05-23

### Added

-   Add `UPGRADING.md` ([**@vweevers**](https://github.com/vweevers))

### Changed

-   Upgrade `abstract-leveldown` to `5.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Upgrade `memdown` to `3.0.0` ([**@vweevers**](https://github.com/vweevers))

## [5.0.1] - 2018-05-19

### Changed

-   Override `_setupIteratorOptions` to not clobber ranges ([**@ralphtheninja**](https://github.com/ralphtheninja), [**@dominictarr**](https://github.com/dominictarr))

## [4.0.1] - 2018-05-19

### Changed

-   Override `_setupIteratorOptions` to not clobber ranges ([**@ralphtheninja**](https://github.com/ralphtheninja), [**@dominictarr**](https://github.com/dominictarr))

## [5.0.0] - 2018-05-13

### Added

-   Add 10 to Travis ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

-   Update `level-errors` to `2.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update `level-codec` to `9.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Removed

-   Remove 4 from Travis ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [4.0.0] - 2018-02-12

### Added

-   Add 9 to Travis ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

-   Update `abstract-leveldown` to `4.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update `memdown` to `2.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Removed

-   Remove 7 from Travis ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [3.0.1] - 2017-12-18

### Added

-   Test that default utf8 encoding stringifies numbers ([**@vweevers**](https://github.com/vweevers))

### Fixed

-   Skip decoding if `options.keys` or `options.values` is false ([**@vweevers**](https://github.com/vweevers))

## [3.0.0] - 2017-11-11

### Added

-   README: add node badge (>= 4) ([**@vweevers**](https://github.com/vweevers))

### Changed

-   Update `abstract-leveldown` to `3.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Removed

-   Remove 0.12 from Travis ([**@vweevers**](https://github.com/vweevers))

## [2.3.4] - 2017-10-24

### Added

-   README: add example of npm installed encoding ([**@vweevers**](https://github.com/vweevers))

## [2.3.3] - 2017-10-22

### Changed

-   README: fix `level-codec` links ([**@vweevers**](https://github.com/vweevers))

## [2.3.2] - 2017-10-22

### Changed

-   README: tweak badges ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   README: add more code examples ([**@vweevers**](https://github.com/vweevers))
-   Update `level-codec` to `8.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Fixed

-   Fix problems related to missing `asBuffer`, `keyAsBuffer` and `valueAsBuffer` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.3.1] - 2017-10-02

### Changed

-   Refactor typings ([**@MeirionHughes**](https://github.com/MeirionHughes))

## [2.3.0] - 2017-09-24

### Added

-   Add default export ([**@zixia**](https://github.com/zixia))

## [2.2.1] - 2017-09-13

### Fixed

-   Fix typings ([**@MeirionHughes**](https://github.com/MeirionHughes))

## [2.2.0] - 2017-09-12

### Added

-   Add Typescript typings ([**@MeirionHughes**](https://github.com/MeirionHughes))

### Changed

-   README: `AbstractLevelDOWN` -> `abstract-leveldown` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update `abstract-leveldown` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.1.5] - 2017-08-18

### Added

-   README: add api docs ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Add basic tests ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

-   Enable Travis for ci ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update dependencies ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Use `safe-buffer` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.1.4] - 2017-01-26

### Fixed

-   Rename methods to `_serializeKey()` and `_serializeValue()` ([**@juliangruber**](https://github.com/juliangruber))

## [2.1.3] - 2017-01-26

### Added

-   Add `_encodeKey()` and `_encodeValue()` id functions ([**@juliangruber**](https://github.com/juliangruber))

## [2.1.2] - 2017-01-26

### Fixed

-   Emit encoding errors in streams too ([**@juliangruber**](https://github.com/juliangruber))

## [2.1.1] - 2017-01-26

### Fixed

-   Return encoding errors on get ([**@juliangruber**](https://github.com/juliangruber))

## [2.1.0] - 2017-01-26

### Added

-   Add support for `approximateSize()` ([**@juliangruber**](https://github.com/juliangruber))

## [2.0.8] - 2017-01-26

### Removed

-   Remove `Iterator.prototype.seek` ([**@juliangruber**](https://github.com/juliangruber))

### Fixed

-   Fix encoding lt/get range options ([**@juliangruber**](https://github.com/juliangruber))

## [2.0.7] - 2017-01-26

### Added

-   Add `'utf8'` as default encoding ([**@juliangruber**](https://github.com/juliangruber))

## [2.0.6] - 2017-01-26

### Fixed

-   Fix `typof` -> `typeof` bug ([**@juliangruber**](https://github.com/juliangruber))

## [2.0.5] - 2017-01-26

### Fixed

-   Fix bug in `iterator._next()` with undefined key or value ([**@juliangruber**](https://github.com/juliangruber))

## [2.0.4] - 2017-01-26

### Changed

-   Update `level-codec` for utf8 fixes ([**@juliangruber**](https://github.com/juliangruber))

## [2.0.3] - 2017-01-26

### Fixed

-   Fix bug with incorrect db ([**@juliangruber**](https://github.com/juliangruber))

## [2.0.2] - 2017-01-26

### Fixed

-   Fix bug with incorrect db and missing new operator ([**@juliangruber**](https://github.com/juliangruber))

## [2.0.1] - 2017-01-26

### Fixed

-   Fix bug with `AbstractChainedBatch` inheritance ([**@juliangruber**](https://github.com/juliangruber))

## [2.0.0] - 2017-01-26

### Changed

-   Version bump ([**@juliangruber**](https://github.com/juliangruber))

## 1.0.0 - 2017-01-26

:seedling: Initial release.

[unreleased]: https://github.com/level/encoding-down/compare/v5.0.4...HEAD

[5.0.4]: https://github.com/level/encoding-down/compare/v5.0.3...v5.0.4

[5.0.3]: https://github.com/level/encoding-down/compare/v5.0.2...v5.0.3

[5.0.2]: https://github.com/level/encoding-down/compare/v5.0.1...v5.0.2

[5.0.1]: https://github.com/level/encoding-down/compare/v5.0.0...v5.0.1

[5.0.0]: https://github.com/level/encoding-down/compare/v4.0.0...v5.0.0

[4.0.1]: https://github.com/level/encoding-down/compare/v4.0.0...v4.0.1

[4.0.0]: https://github.com/level/encoding-down/compare/v3.0.1...v4.0.0

[3.0.1]: https://github.com/level/encoding-down/compare/v3.0.0...v3.0.1

[3.0.0]: https://github.com/level/encoding-down/compare/v2.3.4...v3.0.0

[2.3.4]: https://github.com/level/encoding-down/compare/v2.3.3...v2.3.4

[2.3.3]: https://github.com/level/encoding-down/compare/v2.3.2...v2.3.3

[2.3.2]: https://github.com/level/encoding-down/compare/v2.3.1...v2.3.2

[2.3.1]: https://github.com/level/encoding-down/compare/v2.3.0...v2.3.1

[2.3.0]: https://github.com/level/encoding-down/compare/v2.2.1...v2.3.0

[2.2.1]: https://github.com/level/encoding-down/compare/v2.2.0...v2.2.1

[2.2.0]: https://github.com/level/encoding-down/compare/v2.1.5...v2.2.0

[2.1.5]: https://github.com/level/encoding-down/compare/v2.1.4...v2.1.5

[2.1.4]: https://github.com/level/encoding-down/compare/v2.1.3...v2.1.4

[2.1.3]: https://github.com/level/encoding-down/compare/v2.1.2...v2.1.3

[2.1.2]: https://github.com/level/encoding-down/compare/v2.1.1...v2.1.2

[2.1.1]: https://github.com/level/encoding-down/compare/v2.1.0...v2.1.1

[2.1.0]: https://github.com/level/encoding-down/compare/v2.0.8...v2.1.0

[2.0.8]: https://github.com/level/encoding-down/compare/v2.0.7...v2.0.8

[2.0.7]: https://github.com/level/encoding-down/compare/v2.0.6...v2.0.7

[2.0.6]: https://github.com/level/encoding-down/compare/v2.0.5...v2.0.6

[2.0.5]: https://github.com/level/encoding-down/compare/v2.0.4...v2.0.5

[2.0.4]: https://github.com/level/encoding-down/compare/v2.0.3...v2.0.4

[2.0.3]: https://github.com/level/encoding-down/compare/v2.0.2...v2.0.3

[2.0.2]: https://github.com/level/encoding-down/compare/v2.0.1...v2.0.2

[2.0.1]: https://github.com/level/encoding-down/compare/v2.0.0...v2.0.1

[2.0.0]: https://github.com/level/encoding-down/compare/v1.0.0...v2.0.0
