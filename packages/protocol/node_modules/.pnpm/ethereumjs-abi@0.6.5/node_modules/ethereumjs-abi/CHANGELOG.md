# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/) 
(modification: no type change headlines) and this project adheres to 
[Semantic Versioning](http://semver.org/spec/v2.0.0.html).

## [0.6.5] - 2017-12-07
- Fix tight packing for ``ABI.soliditySHA3``
- Fix ``ABI.solidityPack`` early return on bytes32 input
- Support for multi dim / dynamic arrays
- Support addresses starting with zeroes

[0.6.5]: https://github.com/ethereumjs/ethereumjs-abi/compare/v0.6.4...v0.6.5

## [0.6.4] - 2016-08-29
- Fix length calculation for static arrays and skip enough bytes after them

[0.6.4]: https://github.com/ethereumjs/ethereumjs-abi/compare/v0.6.3...v0.6.4

## [0.6.3] - 2016-08-10
- Support for the (u)``fixedMxN`` datatype
- Fix conversion of type shorthands (int, uint, fixed, ufixed)
- Serpent fixes
- Other bug fixes / improvements

[0.6.3]: https://github.com/ethereumjs/ethereumjs-abi/compare/v0.6.2...v0.6.3

## [0.6.2] - 2016-06-01
- Minor improvements and bug fixes
- Additional tests

[0.6.2]: https://github.com/ethereumjs/ethereumjs-abi/compare/v0.6.1...v0.6.2

## [0.6.1] - 2016-05-19
- Properly decode fixed-length arrays

[0.6.1]: https://github.com/ethereumjs/ethereumjs-abi/compare/v0.6.0...v0.6.1

## [0.6.0] - 2016-04-26
- Introduce ``ABI.stringify``
- Remove signature handling form ``rawEncode``/``rawDecode``
- Added ``ABI.simpleEncode`` and ``ABI.simpleDecode``
- Export ``methodID`` and ``eventID``
- Various improvements

[0.6.0]: https://github.com/ethereumjs/ethereumjs-abi/compare/v0.5.1...v0.6.0

## Older releases:

- [0.5.1](https://github.com/ethereumjs/ethereumjs-abi/compare/v0.5.0...v0.5.1) - 2016-04-12
- [0.5.0](https://github.com/ethereumjs/ethereumjs-abi/compare/v0.4.0...v0.5.0) - 2016-03-25
- [0.4.0](https://github.com/ethereumjs/ethereumjs-abi/compare/v0.3.0...v0.4.0) - 2015-12-17
- [0.3.0](https://github.com/ethereumjs/ethereumjs-abi/compare/v0.2.0...v0.3.0) - 2015-11-30
- [0.2.0](https://github.com/ethereumjs/ethereumjs-abi/compare/v0.1.0...v0.2.0) - 2015-11-25
- 0.1.0 - 2015-11-23


