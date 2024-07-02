# Changelog

## [4.0.1] - 2022-03-06

### Fixed

- Update support matrix ([`c30998f`](https://github.com/Level/supports/commit/c30998f)) (Vincent Weevers)
- Align LICENSE with SPDX ([`1bb9f9c`](https://github.com/Level/supports/commit/1bb9f9c)) (Vincent Weevers)

## [4.0.0] - 2022-01-02

### Removed

- **Breaking:** drop Node.js 10 ([`ad1c533`](https://github.com/Level/supports/commit/ad1c533)) (Vincent Weevers)
- Remove unused properties ([`c894eba`](https://github.com/Level/supports/commit/c894eba)) (Vincent Weevers).

### Fixed

- Align docs with `abstract-level` ([`c894eba`](https://github.com/Level/supports/commit/c894eba)) (Vincent Weevers).

## [3.0.0] - 2021-11-07

_No longer compatible with `abstract-leveldown`, `levelup` or their dependents._

### Changed

- **Breaking:** change `encodings` and `events` to always be objects ([`eb819a6`](https://github.com/Level/supports/commit/eb819a6)) (Vincent Weevers)
- **Breaking:** replace default export with named export ([`7e9e467`](https://github.com/Level/supports/commit/7e9e467)) (Vincent Weevers)

### Added

- Add typings ([`3df48fc`](https://github.com/Level/supports/commit/3df48fc)) (Vincent Weevers).

## [2.1.0] - 2021-10-27

### Changed

- Allow `manifest.encodings` to be an object ([`284e0db`](https://github.com/Level/supports/commit/284e0db)) (Vincent Weevers)

## [2.0.2] - 2021-10-09

### Added

- Add `idempotentOpen` and `passiveOpen` features ([`fc3f1e0`](https://github.com/Level/supports/commit/fc3f1e0)) (Vincent Weevers)
- Add `events` feature ([`22a3837`](https://github.com/Level/supports/commit/22a3837)) (Vincent Weevers)
- Document that `encodings` implies utf8 default ([`d1b6d89`](https://github.com/Level/supports/commit/d1b6d89)) (Vincent Weevers)
- Document `status` values ([`0837a16`](https://github.com/Level/supports/commit/0837a16)) (Vincent Weevers)

### Fixed

- Update support matrices ([`eb92d8b`](https://github.com/Level/supports/commit/eb92d8b), [`ef14920`](https://github.com/Level/supports/commit/ef14920), [`0681a1e`](https://github.com/Level/supports/commit/0681a1e), [`ca2a0e6`](https://github.com/Level/supports/commit/ca2a0e6)) (Vincent Weevers)

## [2.0.1] - 2021-09-24

### Added

- Add features: `getMany`, `keyIterator`, `valueIterator`, `iteratorNextv`, `iteratorAll` ([#11](https://github.com/Level/supports/issues/11)) ([`b44a410`](https://github.com/Level/supports/commit/b44a410)) (Vincent Weevers)

## [2.0.0] - 2021-04-09

### Changed

- **Breaking:** modernize syntax and bump standard ([`5d7cc6d`](https://github.com/Level/supports/commit/5d7cc6d)) ([Level/community#98](https://github.com/Level/community/issues/98)) (Vincent Weevers). Drops support of node 6, node 8 and old browsers.

## [1.0.1] - 2019-10-13

### Added

- Document format of `additionalMethods` ([`192bc9e`](https://github.com/Level/supports/commit/192bc9e)) ([**@vweevers**](https://github.com/vweevers))

### Fixed

- Clone `additionalMethods` to prevent mutation ([#4](https://github.com/Level/supports/issues/4)) ([**@vweevers**](https://github.com/vweevers))

## [1.0.0] - 2019-09-22

:seedling: Initial release.

[4.0.1]: https://github.com/Level/supports/releases/tag/v4.0.1

[4.0.0]: https://github.com/Level/supports/releases/tag/v4.0.0

[3.0.0]: https://github.com/Level/supports/releases/tag/v3.0.0

[2.1.0]: https://github.com/Level/supports/releases/tag/v2.1.0

[2.0.2]: https://github.com/Level/supports/releases/tag/v2.0.2

[2.0.1]: https://github.com/Level/supports/releases/tag/v2.0.1

[2.0.0]: https://github.com/Level/supports/releases/tag/v2.0.0

[1.0.1]: https://github.com/Level/supports/releases/tag/v1.0.1

[1.0.0]: https://github.com/Level/supports/releases/tag/v1.0.0
