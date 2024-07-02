# Changelog

## [1.0.4] - 2024-01-20

### Fixed

- Fix TypeScript definitions of `all()` and `nextv()` ([#67](https://github.com/Level/abstract-level/issues/67)) ([`8e85993`](https://github.com/Level/abstract-level/commit/8e85993), [`9f17757`](https://github.com/Level/abstract-level/commit/9f17757)) (Bryan)

## [1.0.3] - 2022-03-20

### Added

- Document error codes of `classic-level` and `many-level` ([#20](https://github.com/Level/abstract-level/issues/20)) ([`4b3464c`](https://github.com/Level/abstract-level/commit/4b3464c)) (Vincent Weevers)

### Fixed

- Add hidden `abortOnClose` option to iterators ([`2935180`](https://github.com/Level/abstract-level/commit/2935180)) (Vincent Weevers)
- Make internal iterator decoding options enumerable ([`eb08363`](https://github.com/Level/abstract-level/commit/eb08363)) (Vincent Weevers)
- Restore Sauce Labs browser tests ([`90b8816`](https://github.com/Level/abstract-level/commit/90b8816)) (Vincent Weevers)

## [1.0.2] - 2022-03-06

### Fixed

- Fix TypeScript declaration of chained batch `write()` options ([`392b7f7`](https://github.com/Level/abstract-level/commit/392b7f7)) (Vincent Weevers)
- Document the return type of `db.batch()` and add example ([`9739bba`](https://github.com/Level/abstract-level/commit/9739bba)) (Vincent Weevers)

## [1.0.1] - 2022-02-06

### Fixed

- Add `highWaterMarkBytes` option to tests where it matters ([`6b25a91`](https://github.com/Level/abstract-level/commit/6b25a91)) (Vincent Weevers)
- Clarify the meaning of `db.status` ([`2e90b05`](https://github.com/Level/abstract-level/commit/2e90b05)) (Vincent Weevers)
- Use `new` in README examples ([`379503e`](https://github.com/Level/abstract-level/commit/379503e)) (Vincent Weevers).

## [1.0.0] - 2022-01-30

_:seedling: Initial release. If you are upgrading from `abstract-leveldown` please see [`UPGRADING.md`](UPGRADING.md)_

[1.0.4]: https://github.com/Level/abstract-level/releases/tag/v1.0.4

[1.0.3]: https://github.com/Level/abstract-level/releases/tag/v1.0.3

[1.0.2]: https://github.com/Level/abstract-level/releases/tag/v1.0.2

[1.0.1]: https://github.com/Level/abstract-level/releases/tag/v1.0.1

[1.0.0]: https://github.com/Level/abstract-level/releases/tag/v1.0.0
