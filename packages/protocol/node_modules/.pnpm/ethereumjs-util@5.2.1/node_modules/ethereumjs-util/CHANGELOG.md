# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/) 
(modification: no type change headlines) and this project adheres to 
[Semantic Versioning](http://semver.org/spec/v2.0.0.html).


## [5.2.1] - 2020-07-17

This release replaces the native `secp256k1` and `keccak` dependencies with
[ethereum-cryptopgraphy](https://github.com/ethereum/js-ethereum-cryptography)
which doesn't need native compilation.

[5.2.1]: https://github.com/ethereumjs/ethereumjs-util/compare/v5.2.0...v5.2.1

## [5.2.0] - 2018-04-27
- Rename all ``sha3`` hash related constants and functions to ``keccak``, see
  [this](https://github.com/ethereum/EIPs/issues/59) EIP discussion for context
  (tl;dr: Ethereum uses a slightly different hash algorithm then in the official
  ``SHA-3`` standard)
- Renamed constants:
  - ``SHA3_NULL_S`` -> ``KECCAK256_NULL_S``
  - ``SHA3_NULL`` -> ``KECCAK256_NULL``
  - ``SHA3_RLP_ARRAY_S`` -> ``KECCAK256_RLP_ARRAY_S``
  - ``SHA3_RLP_ARRAY`` -> ``KECCAK256_RLP_ARRAY``
  - ``SHA3_RLP_S`` -> ``KECCAK256_RLP_S``
  - ``SHA3_RLP`` -> ``KECCAK256_RLP``
- Renamed functions:
  - ``sha3()`` -> ``keccak()`` (number of bits determined in arguments)
- New ``keccak256()`` alias function for ``keccak(a, 256)``
- The usage of the ``sha``-named versions is now ``DEPRECATED`` and the related 
  constants and functions will be removed on the next major release ``v6.0.0``

[5.2.0]: https://github.com/ethereumjs/ethereumjs-util/compare/v5.1.5...v5.2.0

## [5.1.5] - 2018-02-28
- Fix ``browserify`` issue leading to 3rd-party build problems, PR [#119](https://github.com/ethereumjs/ethereumjs-util/pull/119)

[5.1.5]: https://github.com/ethereumjs/ethereumjs-util/compare/v5.1.4...v5.1.5

## [5.1.4] - 2018-02-03
- Moved to ``ES5`` Node distribution version for easier toolchain integration, PR [#114](https://github.com/ethereumjs/ethereumjs-util/pull/114)
- Updated ``isPrecompile()`` with Byzantium precompile address range, PR [#115](https://github.com/ethereumjs/ethereumjs-util/pull/115)

[5.1.4]: https://github.com/ethereumjs/ethereumjs-util/compare/v5.1.3...v5.1.4

## [5.1.3] - 2018-01-03
- ``ES6`` syntax updates
- Dropped Node ``5`` support
- Moved babel to dev dependencies, switched to ``env`` preset
- Usage of ``safe-buffer`` instead of Node ``Buffer``
- Do not allow capital ``0X`` as valid address in ``isValidAddress()``
- New methods ``zeroAddress()`` and ``isZeroAddress()``
- Updated dependencies

[5.1.3]: https://github.com/ethereumjs/ethereumjs-util/compare/v5.1.2...v5.1.3

## [5.1.2] - 2017-05-31
- Add browserify for ``ES2015`` compatibility
- Fix hex validation

[5.1.2]: https://github.com/ethereumjs/ethereumjs-util/compare/v5.1.1...v5.1.2

## [5.1.1] - 2017-02-10
- Use hex utils from ``ethjs-util``
- Move secp vars into functions
- Dependency updates

[5.1.1]: https://github.com/ethereumjs/ethereumjs-util/compare/v5.1.0...v5.1.1

## [5.1.0] - 2017-02-04
- Fix ``toRpcSig()`` function
- Updated Buffer creation (``Buffer.from``)
- Dependency updates
- Fix npm error
- Use ``keccak`` package instead of ``keccakjs``
- Helpers for ``eth_sign`` RPC call

[5.1.0]: https://github.com/ethereumjs/ethereumjs-util/compare/v5.0.1...v5.1.0

## [5.0.1] - 2016-11-08
- Fix ``bufferToHex()``

[5.0.1]: https://github.com/ethereumjs/ethereumjs-util/compare/v5.0.0...v5.0.1

## [5.0.0] - 2016-11-08
- Added ``isValidSignature()`` (ECDSA signature validation)
- Change ``v`` param in ``ecrecover()`` from ``Buffer`` to ``int`` (breaking change!)
- Fix property alias for setting with initial parameters
- Reject invalid signature lengths for ``fromRpcSig()``
- Fix ``sha3()`` ``width`` param (byte -> bit)
- Fix overflow bug in ``bufferToInt()``

[5.0.0]: https://github.com/ethereumjs/ethereumjs-util/compare/v4.5.0...v5.0.0

## [4.5.0] - 2016-17-12
- Introduced ``toMessageSig()`` and ``fromMessageSig()``

[4.5.0]: https://github.com/ethereumjs/ethereumjs-util/compare/v4.4.1...v4.5.0

## Older releases:

- [4.4.1](https://github.com/ethereumjs/ethereumjs-util/compare/v4.4.0...v4.4.1) - 2016-05-20
- [4.4.0](https://github.com/ethereumjs/ethereumjs-util/compare/v4.3.1...v4.4.0) - 2016-04-26
- [4.3.1](https://github.com/ethereumjs/ethereumjs-util/compare/v4.3.0...v4.3.1) - 2016-04-25
- [4.3.0](https://github.com/ethereumjs/ethereumjs-util/compare/v4.2.0...v4.3.0) - 2016-03-23
- [4.2.0](https://github.com/ethereumjs/ethereumjs-util/compare/v4.1.0...v4.2.0) - 2016-03-18
- [4.1.0](https://github.com/ethereumjs/ethereumjs-util/compare/v4.0.0...v4.1.0) - 2016-03-08
- [4.0.0](https://github.com/ethereumjs/ethereumjs-util/compare/v3.0.0...v4.0.0) - 2016-02-02
- [3.0.0](https://github.com/ethereumjs/ethereumjs-util/compare/v2.0.0...v3.0.0) - 2016-01-20