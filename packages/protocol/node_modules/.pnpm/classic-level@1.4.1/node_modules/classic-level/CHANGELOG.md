# Changelog

## [1.4.1] - 2024-01-20

### Fixed

- Fix race condition in tests ([#90](https://github.com/Level/classic-level/issues/90)) ([`9ff2e82`](https://github.com/Level/classic-level/commit/9ff2e82)) (Matthew Keil).

## [1.4.0] - 2023-11-26

_Not released to npm because of a race issue, which was fixed in 1.4.1._

### Added

- Add opt-in multithreading ([#85](https://github.com/Level/classic-level/issues/85)) ([`7d497a5`](https://github.com/Level/classic-level/commit/7d497a5)) (Matthew Keil).

## [1.3.0] - 2023-04-07

### Changed

- Refactor some pointer usage ([#25](https://github.com/Level/classic-level/issues/25)) ([`d6437b4`](https://github.com/Level/classic-level/commit/d6437b4)) (Robert Nagy)
- Refactor: handle view encoding (Uint8Array) natively ([#43](https://github.com/Level/classic-level/issues/43)) ([`b9fd5e9`](https://github.com/Level/classic-level/commit/b9fd5e9)) (Vincent Weevers)
- Bump and unlock `napi-macros` from 2.0.0 to 2.2.2 ([#58](https://github.com/Level/classic-level/issues/58)) ([`8a4717b`](https://github.com/Level/classic-level/commit/8a4717b)) (Vincent Weevers).

### Fixed

- Swap linux-arm build to use `linux-arm64-lts` ([#71](https://github.com/Level/classic-level/issues/71)) ([`5ea74ab`](https://github.com/Level/classic-level/commit/5ea74ab)) (Cody Swendrowski)
- Add `openssl_fips` variable to gyp bindings ([#72](https://github.com/Level/classic-level/issues/72)) ([`b3f8517`](https://github.com/Level/classic-level/commit/b3f8517)) (Cody Swendrowski).

## [1.2.0] - 2022-03-25

### Added

- Yield `LEVEL_LOCKED` error when lock is held ([#8](https://github.com/Level/classic-level/issues/8)) ([`aa975de`](https://github.com/Level/classic-level/commit/aa975de)) (Vincent Weevers)

### Fixed

- Fix `getMany()` memory leak ([#9](https://github.com/Level/classic-level/issues/9)) ([`00364c7`](https://github.com/Level/classic-level/commit/00364c7)) (Vincent Weevers).

## [1.1.0] - 2022-03-06

### Added

- Create location directory recursively ([#6](https://github.com/Level/classic-level/issues/6)) ([`1ba0b69`](https://github.com/Level/classic-level/commit/1ba0b69)) (Vincent Weevers)

### Fixed

- Fix TypeScript type declarations ([`a79fe82`](https://github.com/Level/classic-level/commit/a79fe82)) (Vincent Weevers)
- Document the return type of `db.batch()` and add example ([`a909ea6`](https://github.com/Level/classic-level/commit/a909ea6)) (Vincent Weevers).

## [1.0.0] - 2022-03-04

_:seedling: Initial release. If you are upgrading from `leveldown` please see [`UPGRADING.md`](UPGRADING.md)._

[1.4.1]: https://github.com/Level/classic-level/releases/tag/v1.4.1

[1.4.0]: https://github.com/Level/classic-level/releases/tag/v1.4.0

[1.3.0]: https://github.com/Level/classic-level/releases/tag/v1.3.0

[1.2.0]: https://github.com/Level/classic-level/releases/tag/v1.2.0

[1.1.0]: https://github.com/Level/classic-level/releases/tag/v1.1.0

[1.0.0]: https://github.com/Level/classic-level/releases/tag/v1.0.0
