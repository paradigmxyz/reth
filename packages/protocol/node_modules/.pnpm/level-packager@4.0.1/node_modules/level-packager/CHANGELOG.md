# Changelog

## [Unreleased]

## [4.0.1] - 2018-06-23

### Changed

-   Use `var` instead of `let` ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Added

-   Add `remark` tooling ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Add `'use strict'` to all abstract tests ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Removed

-   Remove `contributors` from `package.json` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [4.0.0] - 2018-06-13

### Changed

-   Rewrite `test.js` to test `level-packager` api ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [3.1.0] - 2018-05-28

### Changed

-   Split up tests into `abstract/*-test.js` ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Removed

-   Remove `.jshintrc` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [3.0.0] - 2018-05-23

### Added

-   Add node 10 to Travis ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Add `UPGRADING.md` ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

-   Update `standard` to `^11.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update `encoding-down` to `~5.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update `leveldown` to `^4.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update `levelup` to `^3.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Change `License & Copyright` to `License` in README ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Replace `const` with `var` for IE10 support ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Removed

-   Remove node 4 from Travis ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.1.1] - 2018-02-13

### Added

-   Travis: add 9 ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

-   Update `encoding-down` to `~4.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update `leveldown` to `^3.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update copyright year to 2018 ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Fixed

-   Test: clean up `level-test-*` dbs after tests are done ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.1.0] - 2017-12-13

### Added

-   Add `standard` for linting ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

-   Attach `.errors` from `levelup` to `Level` constructor ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.0.2] - 2017-11-11

### Changed

-   Update `encoding-down` to `~3.0.0` ([**@vweevers**](https://github.com/vweevers))
-   README: update node badge ([**@vweevers**](https://github.com/vweevers))

### Fixed

-   Travis: restore node 4 ([**@vweevers**](https://github.com/vweevers))

## [2.0.1] - 2017-10-12

### Added

-   Test that encoding options default to utf8 ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Test that `.keyEncoding` and `.valueEncoding` are passed to `encoding-down` ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Fixed

-   Fix encoding options to `encoding-down` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.0.0] - 2017-10-11

### Added

-   README: add `level` badge ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

-   Update `levelup` to `^2.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update `encoding-down` to `~2.3.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update `leveldown` to `^2.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   README: update npm badges to similar badge style ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Removed

-   README: Remove Greenkeeper badge ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.0.0-rc3] - 2017-09-16

### Changed

-   Update `levelup` to `2.0.0-rc3` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update `leveldown` to `^1.8.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.0.0-rc2] - 2017-09-12

### Changed

-   Update `levelup` to `2.0.0-rc2` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update `encoding-down` to `~2.2.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.0.0-rc1] - 2017-09-04

### Added

-   Travis: add 8 ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   README: add Greenkeeper badge ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   README: add node badge ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

-   README: steer away from `LevelDOWN` to `abstract-leveldown` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update copyright year to 2017 ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Removed

-   Travis: remove 0.12, 4, 5 and 7 ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Fixed

## [1.2.1] - 2016-12-27

### Added

-   Travis: add 6 and 7 ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

-   Travis: use gcc 4.8 ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Removed

-   Travis: remove 0.10, 1.0, 1.8, 2 and 3 ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [1.2.0] - 2015-11-27

### Added

-   Add dependency badge ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Travis: add 1.0, 2, 3, 4 and 5 ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

-   Update `levelup` to `~1.3.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update `leveldown` to `^1.4.2` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [1.1.0] - 2015-06-09

### Changed

-   Update `levelup` to `~1.2.0` ([**@mcollina**](https://github.com/mcollina))
-   Update `leveldown` to `~1.2.2` ([**@mcollina**](https://github.com/mcollina))

## [1.0.0] - 2015-05-19

### Changed

-   README: add link to `level/community` repo ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [1.0.0-0] - 2015-05-16

### Added

-   Add Travis ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Add `leveldown` dev dependency ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

-   Update `levelup` to `~1.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Run tests using `packager(leveldown)` ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Removed

-   Remove `level` dependency ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [0.19.7] - 2015-05-10

### Added

-   Add `level-test-*` to `.gitignore` ([**@juliangruber**](https://github.com/juliangruber))

### Changed

-   Run the tests if they are not required ([**@juliangruber**](https://github.com/juliangruber))
-   Rename the repository to `packager` ([**@juliangruber**](https://github.com/juliangruber))

## [0.19.6] - 2015-05-10

### Fixed

-   Fix incorrect options logic ([**@juliangruber**](https://github.com/juliangruber))

## [0.19.5] - 2015-05-10

### Fixed

-   Fixed bug with missing opening curly brace ([**@juliangruber**](https://github.com/juliangruber))

## [0.19.4] - 2015-05-10

### Changed

-   Use `typeof` instead of `util.isFunction()` ([**@juliangruber**](https://github.com/juliangruber))

## [0.19.3] - 2015-05-10

### Fixed

-   Fix missing closing parenthesis ([**@juliangruber**](https://github.com/juliangruber))

## [0.19.2] - 2015-05-10

### Fixed

-   Fix missing closing parenthesis ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [0.19.1] - 2015-05-10

### Fixed

-   `null` options should not be treated as object ([**@deian**](https://github.com/deian))

## [0.19.0] - 2015-05-04

### Changed

-   Plain MIT license ([**@andrewrk**](https://github.com/andrewrk))
-   README: update logo and copyright year ([**@ralphtheninja**](https://github.com/ralphtheninja))
-   Update `levelup` to `~0.19.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [0.18.0] - 2013-11-18

### Changed

-   Bumped version ([**@rvagg**](https://github.com/rvagg))

## [0.17.0-5] - 2013-10-12

### Changed

-   Clean up debugging noise ([**@rvagg**](https://github.com/rvagg))

## [0.17.0-4] - 2013-10-12

### Removed

-   Remove `copy()` ([**@rvagg**](https://github.com/rvagg))

### Fixed

-   Fix `repair()` and `destroy()` ([**@rvagg**](https://github.com/rvagg))

## [0.17.0-3] - 2013-10-12

### Fixed

-   Made tests compatible with node 0.8 ([**@rvagg**](https://github.com/rvagg))

## [0.17.0-2] - 2013-10-12

### Added

-   Add options to exported tests to handle memdown ([**@rvagg**](https://github.com/rvagg))

### Changed

-   README: `level` -> `level-packager` ([**@rvagg**](https://github.com/rvagg))

## [0.17.0-1] - 2013-10-09

### Removed

-   Remove `tape` from devDependencies, allow callers to pass in custom test function ([**@rvagg**](https://github.com/rvagg))

## 0.17.0 - 2013-10-09

:seedling: Initial release.

[unreleased]: https://github.com/level/packager/compare/v4.0.1...HEAD

[4.0.1]: https://github.com/level/packager/compare/v4.0.0...v4.0.1

[4.0.0]: https://github.com/level/packager/compare/v3.1.0...v4.0.0

[3.1.0]: https://github.com/level/packager/compare/v3.0.0...v3.1.0

[3.0.0]: https://github.com/level/packager/compare/v2.1.1...v3.0.0

[2.1.1]: https://github.com/level/packager/compare/v2.1.0...v2.1.1

[2.1.0]: https://github.com/level/packager/compare/v2.0.2...v2.1.0

[2.0.2]: https://github.com/level/packager/compare/v2.0.1...v2.0.2

[2.0.1]: https://github.com/level/packager/compare/v2.0.0...v2.0.1

[2.0.0]: https://github.com/level/packager/compare/v2.0.0-rc3...v2.0.0

[2.0.0-rc3]: https://github.com/level/packager/compare/v2.0.0-rc2...v2.0.0-rc3

[2.0.0-rc2]: https://github.com/level/packager/compare/v2.0.0-rc1...v2.0.0-rc2

[2.0.0-rc1]: https://github.com/level/packager/compare/v1.2.1...v2.0.0-rc1

[1.2.1]: https://github.com/level/packager/compare/v1.2.0...v1.2.1

[1.2.0]: https://github.com/level/packager/compare/v1.1.0...v1.2.0

[1.1.0]: https://github.com/level/packager/compare/v1.0.0...v1.1.0

[1.0.0]: https://github.com/level/packager/compare/v1.0.0-0...v1.0.0

[1.0.0-0]: https://github.com/level/packager/compare/v0.19.7...v1.0.0-0

[0.19.7]: https://github.com/level/packager/compare/v0.19.6...v0.19.7

[0.19.6]: https://github.com/level/packager/compare/v0.19.5...v0.19.6

[0.19.5]: https://github.com/level/packager/compare/v0.19.4...v0.19.5

[0.19.4]: https://github.com/level/packager/compare/v0.19.3...v0.19.4

[0.19.3]: https://github.com/level/packager/compare/v0.19.2...v0.19.3

[0.19.2]: https://github.com/level/packager/compare/v0.19.1...v0.19.2

[0.19.1]: https://github.com/level/packager/compare/v0.19.0...v0.19.1

[0.19.0]: https://github.com/level/packager/compare/0.18.0...v0.19.0

[0.18.0]: https://github.com/level/packager/compare/0.17.0-5...0.18.0

[0.17.0-5]: https://github.com/level/packager/compare/0.17.0-4...0.17.0-5

[0.17.0-4]: https://github.com/level/packager/compare/0.17.0-3...0.17.0-4

[0.17.0-3]: https://github.com/level/packager/compare/0.17.0-2...0.17.0-3

[0.17.0-2]: https://github.com/level/packager/compare/0.17.0-1...0.17.0-2

[0.17.0-1]: https://github.com/level/packager/compare/0.17.0...0.17.0-1
