# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
(modification: no type change headlines) and this project adheres to
[Semantic Versioning](http://semver.org/spec/v2.0.0.html).

## [3.0.0] - 2019-01-14

First **TypeScript** based release of the library together with a switch to an `ES6`
class structure of the `Account` class. `TypeScript` handles `ES6` transpilation
[a bit differently](https://github.com/Microsoft/TypeScript/issues/2719) (at the
end: cleaner) than `babel` so `require` syntax of the library slightly changes to:

```javascript
let Account = require('ethereumjs-account').default
```

The library now also comes with a **type declaration file** distributed along with the package published.

- Migration of code base and toolchain to `TypeScript`, PR [#27](https://github.com/ethereumjs/ethereumjs-account/pull/27)
- Updated `ethereumjs-util` dependency to `v6.0.0`

[3.0.0]: https://github.com/ethereumjs/ethereumjs-account/compare/v2.0.5...v3.0.0

## [2.0.5] - 2018-05-08

- Fixes a bug for contract code stored with level DB, PR [#5](https://github.com/ethereumjs/ethereumjs-account/pull/5)
- Added `safe-buffer` dependency for backwards compatibility, PR [#14](https://github.com/ethereumjs/ethereumjs-account/pull/14)
- Code examples in README for `getCode`/`setCode` and `getStorage`/`setStorage`, PR [#19](https://github.com/ethereumjs/ethereumjs-account/pull/19)
- Added test coverage
- Updated dependencies

[2.0.5]: https://github.com/ethereumjs/ethereumjs-account/compare/v2.0.2...v2.0.5

## [2.0.2] - 2016-03-01

- Update dependencies to install on windows

[2.0.2]: https://github.com/ethereumjs/ethereumjs-account/compare/v2.0.1...v2.0.2

## [2.0.1] - 2016-01-17

- Use `SHA_*_S` for faster comparison

[2.0.1]: https://github.com/ethereumjs/ethereumjs-account/compare/v2.0.0...v2.0.1

## [2.0.0] - 2016-01-06

- Improved documentation
- Simplified `getCode()`
- Added tests

[2.0.0]: https://github.com/ethereumjs/ethereumjs-account/compare/v1.0.5...v2.0.0

## Older releases:

- [1.0.5](https://github.com/ethereumjs/ethereumjs-account/compare/1.0.3...v1.0.5) - 2015-11-27
- 1.0.3 - 2015-09-24
