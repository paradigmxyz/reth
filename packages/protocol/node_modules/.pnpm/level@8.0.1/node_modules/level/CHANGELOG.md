# Changelog

## [8.0.1] - 2024-01-27

### Fixed

- Explicitly depend on abstract-level for TypeScript ([#241](https://github.com/Level/level/issues/241)) ([`c501868`](https://github.com/Level/level/commit/c501868)) (Hanxx).

## [8.0.0] - 2022-03-25

_If you are upgrading: please see [`UPGRADING.md`](UPGRADING.md)._

### Changed

- **Breaking:** switch to `classic-level` and `browser-level` ([#215](https://github.com/Level/level/issues/215)) ([`ad22b21`](https://github.com/Level/level/commit/ad22b21)) (Vincent Weevers).

## [7.0.1] - 2021-10-02

### Added

- Document new features ([#207](https://github.com/Level/level/issues/207)) ([`ad8f924`](https://github.com/Level/level/commit/ad8f924)) (Vincent Weevers)

### Fixed

- Bump dependencies to prevent dedupe ([`7083ec6`](https://github.com/Level/level/commit/7083ec6)) (Vincent Weevers)
- Clarify `close()` documentation ([#197](https://github.com/Level/level/issues/197)) ([`c82fdbc`](https://github.com/Level/level/commit/c82fdbc)) (Vincent Weevers)

## [7.0.0] - 2021-04-17

_If you are upgrading: please see [`UPGRADING.md`](UPGRADING.md)._

### Changed

- **Breaking:** bump `leveldown` and `level-packager` ([`53bd922`](https://github.com/Level/level/commit/53bd922)) (Vincent Weevers)
- **Breaking:** bump `level-js` from 5.x to 6.x ([#194](https://github.com/Level/level/issues/194)) ([`1f6c603`](https://github.com/Level/level/commit/1f6c603)) (Alex Potsides)
- **Breaking:** modernize syntax ([Level/community#98](https://github.com/Level/community/issues/98)) ([`d001b2c`](https://github.com/Level/level/commit/d001b2c)) (Vincent Weevers)
- Add `files` to `package.json` and remove `.npmignore` ([`329e1f5`](https://github.com/Level/level/commit/329e1f5)) (Vincent Weevers)

### Added

- Document chained batch encoding options ([Level/levelup#633](https://github.com/Level/levelup/issues/633)) ([`0b3c11d`](https://github.com/Level/level/commit/0b3c11d)) (Vincent Weevers)
- Document the `clear` event ([Level/community#79](https://github.com/Level/community/issues/79)) ([`52314bf`](https://github.com/Level/level/commit/52314bf)) (Vincent Weevers)

### Removed

- **Breaking:** drop node 8 ([Level/community#98](https://github.com/Level/community/issues/98)) ([`f8a0047`](https://github.com/Level/level/commit/f8a0047), [`31317a6`](https://github.com/Level/level/commit/31317a6)) (Vincent Weevers)
- Remove legacy range options from README ([Level/community#86](https://github.com/Level/community/issues/86)) ([`e56c6b1`](https://github.com/Level/level/commit/e56c6b1)) (Vincent Weevers)

## [6.0.1] - 2020-03-04

### Changed

- Switch from `opencollective-postinstall` to npm `funding` ([#173](https://github.com/Level/level/issues/173)) ([**@Richienb**](https://github.com/Richienb))
- Upgrade `nyc` devDependency from `^14.0.0` to `^15.0.0` ([#169](https://github.com/Level/level/issues/169)) ([**@vweevers**](https://github.com/vweevers))
- Upgrade `airtap` devDependency from `^2.0.1` to `^3.0.0` ([#171](https://github.com/Level/level/issues/171)) ([**@vweevers**](https://github.com/vweevers))

## [6.0.0] - 2019-10-19

_If you are upgrading: please see [`UPGRADING.md`](UPGRADING.md)._

### Changed

- **Breaking:** upgrade `level-js` from `^4.0.0` to `^5.0.0` ([#158](https://github.com/Level/level/issues/158)) ([**@vweevers**](https://github.com/vweevers))
- Upgrade `hallmark` devDependency from `^0.1.0` to `^2.0.0` ([#152](https://github.com/Level/level/issues/152), [#157](https://github.com/Level/level/issues/157)) ([**@vweevers**](https://github.com/vweevers))
- Upgrade `standard` devDependency from `^12.0.0` to `^14.0.0` ([#151](https://github.com/Level/level/issues/151), [#155](https://github.com/Level/level/issues/155)) ([**@vweevers**](https://github.com/vweevers))
- Upgrade `nyc` devDependency from `^13.2.0` to `^14.0.0` ([#147](https://github.com/Level/level/issues/147)) ([**@vweevers**](https://github.com/vweevers))

### Added

- Document manifest, `iterator()` and `clear()` ([#162](https://github.com/Level/level/issues/162)) ([**@vweevers**](https://github.com/vweevers))
- Add links to `browserify-starter` and `webpack-starter` ([`6ff1802`](https://github.com/Level/level/commit/6ff1802)) ([**@vweevers**](https://github.com/vweevers))

### Fixed

- Bump `leveldown` and `level-packager` to prevent dedupe ([`5efdc82`](https://github.com/Level/level/commit/5efdc82), [`a9656c3`](https://github.com/Level/level/commit/a9656c3), [`97cb31c`](https://github.com/Level/level/commit/97cb31c)) ([**@vweevers**](https://github.com/vweevers))

## [5.0.1] - 2019-03-29

### Fixed

- Temporarily skip `hallmark` test because it breaks CITGM ([#145](https://github.com/Level/level/issues/145)) ([**@vweevers**](https://github.com/vweevers))

## [5.0.0] - 2019-03-29

_If you are upgrading: please see [`UPGRADING.md`](UPGRADING.md)._

### Changed

- Upgrade `leveldown` from `^4.0.0` to `^5.0.0` ([#133](https://github.com/Level/level/issues/133), [#144](https://github.com/Level/level/issues/144)) ([**@vweevers**](https://github.com/vweevers))
- Upgrade `level-packager` from `^3.0.0` to `^5.0.0` ([#113](https://github.com/Level/level/issues/113), [#131](https://github.com/Level/level/issues/131)) ([**@ralphtheninja**](https://github.com/ralphtheninja), [**@vweevers**](https://github.com/vweevers))
- Prefer `var` over `const` in README ([`f032b6c`](https://github.com/Level/level/commit/f032b6c)) ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Upgrade `standard` devDependency from `^11.0.0` to `^12.0.0` ([#118](https://github.com/Level/level/issues/118)) ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Tweak copyright years for less maintenance ([`0b9c8ad`](https://github.com/Level/level/commit/0b9c8ad)) ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Tweak Open Collective documentation ([#137](https://github.com/Level/level/issues/137)) ([**@vweevers**](https://github.com/vweevers))
- Apply common project tweaks ([#136](https://github.com/Level/level/issues/136), [#138](https://github.com/Level/level/issues/138)) ([**@vweevers**](https://github.com/vweevers))
- Add `.travis.yml` and `appveyor.yml` to `.npmignore` ([`7b5c340`](https://github.com/Level/level/commit/7b5c340)) ([**@vweevers**](https://github.com/vweevers))

### Added

- Integrate `level-js` for browser support ([#135](https://github.com/Level/level/issues/135)) ([**@vweevers**](https://github.com/vweevers))
- Add appveyor ([#112](https://github.com/Level/level/issues/112)) ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Enable OSX on Travis ([#111](https://github.com/Level/level/issues/111)) ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Add `nyc` and `coveralls` devDependencies ([#115](https://github.com/Level/level/issues/115), [#143](https://github.com/Level/level/issues/143)) ([**@ralphtheninja**](https://github.com/ralphtheninja), [**@vweevers**](https://github.com/vweevers))
- Add `hallmark` devDependency ([#134](https://github.com/Level/level/issues/134)) ([**@vweevers**](https://github.com/vweevers))
- Add note about Rollup to `README.md` ([#139](https://github.com/Level/level/issues/139)) ([**@vweevers**](https://github.com/vweevers))

### Removed

- Remove node 6 and 9 ([#129](https://github.com/Level/level/issues/129), [`2bf1d3f`](https://github.com/Level/level/commit/2bf1d3f)) ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Remove contributors from `package.json` ([`f37252d`](https://github.com/Level/level/commit/f37252d)) ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [4.0.0] - 2018-05-23

_If you are upgrading: please see [`UPGRADING.md`](UPGRADING.md)._

### Changed

- Update `leveldown` to `^4.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Update `level-packager` to `^3.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Switch to `opencollective-postinstall` ([**@mateodelnorte**](https://github.com/mateodelnorte))

### Removed

- Remove node 4 from Travis ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [3.0.2] - 2018-05-23

### Changed

- Switch to `opencollective-postinstall` ([**@mateodelnorte**](https://github.com/mateodelnorte))

## [3.0.1] - 2018-05-05

### Added

- Travis: add 10 ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

- Update `standard` to `^11.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Fix typo in README ([**@rasmuserik**](https://github.com/rasmuserik))

### Fixed

- Fix postinstall failures with OpenCollective ([**@vweevers**](https://github.com/vweevers))

## [3.0.0] - 2018-02-17

_If you are upgrading: please see [`UPGRADING.md`](UPGRADING.md)._

### Added

- Travis: add 9 ([**@ralphtheninja**](https://github.com/ralphtheninja))
- README: add table of contents ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

- Update `leveldown` to `^3.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.1.2] - 2018-01-26

### Added

- Add OpenCollective ([**@monkeywithacupcake**](https://github.com/monkeywithacupcake))

### Changed

- README: change Travis badge from png to svg ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.1.1] - 2017-12-13

### Changed

- README: document `.errors` property ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.1.0] - 2017-12-06

### Changed

- Update `level-packager` to `^2.0.2` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Update `leveldown` to `^2.1.1` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.0.1] - 2017-11-11

### Changed

- Restore node 4 ([**@vweevers**](https://github.com/vweevers))

## [2.0.0] - 2017-10-17

_If you are upgrading: please see [`UPGRADING.md`](UPGRADING.md)._

### Added

- Add `standard` for linting ([**@ralphtheninja**](https://github.com/ralphtheninja))
- README: copy over docs from `levelup` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- README: add node badge ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

- Update `level-packager` to `~2.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Update `leveldown` to `~2.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.0.0-rc3] - 2017-09-16

### Changed

- Update `level-packager` to `2.0.0-rc3` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Update `leveldown` to `~1.8.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.0.0-rc2] - 2017-09-12

### Changed

- Update `level-packager` to `2.0.0-rc2` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Update `leveldown` to `~1.7.2` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [2.0.0-rc1] - 2017-09-06

### Added

- README: add Greenkeeper badge ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Travis: add 8 ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

- Update `level-packager` to `2.0.0-rc1` ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Removed

- Travis: remove 0.12, 4, 5, and 7 ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [1.7.0] - 2017-05-17

### Changed

- Update `leveldown` to `~1.7.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [1.6.0] - 2017-02-06

### Added

- Travis: add 7 ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

- Update copyright year to 2017 ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Update `leveldown` to `~1.6.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Removed

- Travis: remove 0.10 ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [1.5.0] - 2016-10-16

### Added

- Travis: add 6 ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

- Use gcc 4.8 on Travis
- Update `leveldown` to `~1.5.0` ([**@juliangruber**](https://github.com/juliangruber))

### Removed

- Travis: remove 1.0, 1.8, 2 and 3 ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [1.4.0] - 2015-11-27

### Added

- Travis: add 1.0, 2, 3, 4 and 5 ([**@ralphtheninja**](https://github.com/ralphtheninja))
- README: add dependency badge ([**@ralphtheninja**](https://github.com/ralphtheninja))

### Changed

- Update `level-packager` to `~1.2.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [1.3.0] - 2015-07-29

### Changed

- Update `leveldown` to `~1.4.0` ([**@ArtskydJ**](https://github.com/ArtskydJ))

## [1.2.0] - 2015-06-24

### Changed

- Update `level-packager` to `~1.1.0` ([**@timoxley**](https://github.com/timoxley))
- Update `leveldown` to `~1.3.0` ([**@timoxley**](https://github.com/timoxley))

## [1.1.0] - 2015-06-02

### Changed

- Update `leveldown` to `~1.2.2` for prebuilt binaries ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [1.0.0] - 2015-05-19

### Changed

- Update `level-packager` to `~1.0.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [1.0.0-0] - 2015-05-16

### Changed

- Use `nvm` again on Travis ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Moved `CONTRIBUTING.md` and contributors to `level/community` repository ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Use `level-packager@next` ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Update `leveldown` to `~1.0.6` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [0.19.1] - 2015-05-05

### Changed

- Use `n` instead of `nvm` on Travis for iojs support on native modules ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [0.19.0] - 2015-05-05

### Changed

- Switch to plain MIT license ([**@andrewrk**](https://github.com/andrewrk))
- README: update nodeico badge ([**@rvagg**](https://github.com/rvagg))
- README: update logo and copyright ([**@ralphtheninja**](https://github.com/ralphtheninja))
- Update `level-packager` to `~0.19.0` ([**@ralphtheninja**](https://github.com/ralphtheninja))

## [0.18.0] - 2013-11-18

### Added

- Add Travis ([**@rvagg**](https://github.com/rvagg))

### Changed

- Update `level-packager` to `~0.18.0` ([**@rvagg**](https://github.com/rvagg))
- Update `leveldown` to `~0.10.0` ([**@rvagg**](https://github.com/rvagg))

## [0.17.0] - 2013-10-01

_0.17.0 and 0.17.0-1 are listed out of order here, due to 0.17.0-1 not adhering to the semver rules that we follow today._

### Changed

- Update `levelup` to `~0.17.0` ([**@rvagg**](https://github.com/rvagg))
- Update `leveldown` to `~0.9.0` ([**@rvagg**](https://github.com/rvagg))

## [0.17.0-1] - 2013-10-09

### Changed

- Use `level-packager` instead of `levelup` ([**@rvagg**](https://github.com/rvagg))
- Run tests in `level-packager` using `tape` ([**@rvagg**](https://github.com/rvagg))

## [0.16.0] - 2013-09-10

### Added

- Add [**@substack**](https://github.com/substack) to contributors ([**@rvagg**](https://github.com/rvagg))

### Changed

- Update `levelup` to `~0.16.0` ([**@rvagg**](https://github.com/rvagg))
- Update repository and homepage in `package.json` ([**@rvagg**](https://github.com/rvagg))

## [0.15.0] - 2013-08-26

### Changed

- README: tweaks ([**@rvagg**](https://github.com/rvagg))
- Update `levelup` to `~0.15.0` ([**@rvagg**](https://github.com/rvagg))
- Update `leveldown` to `~0.8.0` ([**@rvagg**](https://github.com/rvagg))

## [0.14.0] - 2013-08-19

### Added

- README: add npm downloads badge ([**@rvagg**](https://github.com/rvagg))

### Changed

- Update `levelup` to `~0.14.0` ([**@rvagg**](https://github.com/rvagg))

## [0.13.0] - 2013-08-11

### Changed

- Update `levelup` to `~0.13.0` ([**@rvagg**](https://github.com/rvagg))
- Update `leveldown` to `~0.7.0` ([**@rvagg**](https://github.com/rvagg))

## [0.12.0] - 2013-07-25

### Changed

- Update `levelup` to `~0.12.0` ([**@rvagg**](https://github.com/rvagg))
- Update `leveldown` to `~0.6.2` ([**@rvagg**](https://github.com/rvagg))

## [0.11.0] - 2013-07-17

### Added

- Add [**@pgte**](https://github.com/pgte) to contributors ([**@rvagg**](https://github.com/rvagg))
- README: add npm badge ([**@rvagg**](https://github.com/rvagg))

### Changed

- Update `levelup` to `~0.11.0` ([**@rvagg**](https://github.com/rvagg))

## [0.10.0] - 2013-06-14

### Changed

- Update `levelup` to `~0.10.0` ([**@rvagg**](https://github.com/rvagg))
- Update `leveldown` to `~0.6.0` ([**@rvagg**](https://github.com/rvagg))

## [0.9.0] - 2013-05-27

### Changed

- Update `levelup` to `~0.9.0` ([**@rvagg**](https://github.com/rvagg))
- Update `leveldown` to `~0.5.0` ([**@rvagg**](https://github.com/rvagg))

## [0.8.0] - 2013-05-19

:seedling: Initial release.

[8.0.1]: https://github.com/Level/level/releases/tag/v8.0.1

[8.0.0]: https://github.com/Level/level/releases/tag/v8.0.0

[7.0.1]: https://github.com/Level/level/releases/tag/v7.0.1

[7.0.0]: https://github.com/Level/level/releases/tag/v7.0.0

[6.0.1]: https://github.com/Level/level/releases/tag/v6.0.1

[6.0.0]: https://github.com/Level/level/releases/tag/v6.0.0

[5.0.1]: https://github.com/Level/level/releases/tag/v5.0.1

[5.0.0]: https://github.com/Level/level/releases/tag/v5.0.0

[4.0.0]: https://github.com/Level/level/releases/tag/v4.0.0

[3.0.2]: https://github.com/Level/level/releases/tag/v3.0.2

[3.0.1]: https://github.com/Level/level/releases/tag/v3.0.1

[3.0.0]: https://github.com/Level/level/releases/tag/v3.0.0

[2.1.2]: https://github.com/Level/level/releases/tag/v2.1.2

[2.1.1]: https://github.com/Level/level/releases/tag/v2.1.1

[2.1.0]: https://github.com/Level/level/releases/tag/v2.1.0

[2.0.1]: https://github.com/Level/level/releases/tag/v2.0.1

[2.0.0]: https://github.com/Level/level/releases/tag/v2.0.0

[2.0.0-rc3]: https://github.com/Level/level/releases/tag/v2.0.0-rc3

[2.0.0-rc2]: https://github.com/Level/level/releases/tag/v2.0.0-rc2

[2.0.0-rc1]: https://github.com/Level/level/releases/tag/v2.0.0-rc1

[1.7.0]: https://github.com/Level/level/releases/tag/v1.7.0

[1.6.0]: https://github.com/Level/level/releases/tag/v1.6.0

[1.5.0]: https://github.com/Level/level/releases/tag/v1.5.0

[1.4.0]: https://github.com/Level/level/releases/tag/v1.4.0

[1.3.0]: https://github.com/Level/level/releases/tag/v1.3.0

[1.2.0]: https://github.com/Level/level/releases/tag/v1.2.0

[1.1.0]: https://github.com/Level/level/releases/tag/v1.1.0

[1.0.0]: https://github.com/Level/level/releases/tag/v1.0.0

[1.0.0-0]: https://github.com/Level/level/releases/tag/v1.0.0-0

[0.19.1]: https://github.com/Level/level/releases/tag/v0.19.1

[0.19.0]: https://github.com/Level/level/releases/tag/v0.19.0

[0.18.0]: https://github.com/Level/level/releases/tag/0.18.0

[0.17.0]: https://github.com/Level/level/releases/tag/0.17.0

[0.17.0-1]: https://github.com/Level/level/releases/tag/0.17.0-1

[0.16.0]: https://github.com/Level/level/releases/tag/0.16.0

[0.15.0]: https://github.com/Level/level/releases/tag/0.15.0

[0.14.0]: https://github.com/Level/level/releases/tag/0.14.0

[0.13.0]: https://github.com/Level/level/releases/tag/0.13.0

[0.12.0]: https://github.com/Level/level/releases/tag/0.12.0

[0.11.0]: https://github.com/Level/level/releases/tag/0.11.0

[0.10.0]: https://github.com/Level/level/releases/tag/0.10.0

[0.9.0]: https://github.com/Level/level/releases/tag/0.9.0

[0.8.0]: https://github.com/Level/level/releases/tag/0.8.0
