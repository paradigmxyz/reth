# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/) 
(modification: no type change headlines) and this project adheres to 
[Semantic Versioning](http://semver.org/spec/v2.0.0.html).


## [0.2.0] - 2018-07-19
- Added successful test case for each operation, PR[#19](https://github.com/ethereumjs/rustbn.js/pull/19)
- Better Javascript API, PR[#20](https://github.com/ethereumjs/rustbn.js/pull/20) - breaking change
- Support for Rust 1.24, PR[#22](https://github.com/ethereumjs/rustbn.js/pull/22)

[0.2.0]: https://github.com/ethereumjs/rustbn.js/compare/v0.1.2...v0.2.0

## [0.1.2] - 2018-02-09
- Added ``Cargo`` flag to avoid generated code catching all exceptions, PR [#11](https://github.com/ethereumjs/rustbn.js/pull/11)
- More robust build with ``sed`` validation, PR [#14](https://github.com/ethereumjs/rustbn.js/pull/14)
- Fixed license and authors in ``package.json``, PR [#15](https://github.com/ethereumjs/rustbn.js/pull/15)

[0.1.2]: https://github.com/ethereumjs/rustbn.js/compare/v0.1.1...v0.1.2

## [0.1.1] - 2017-10-26
- Avoid exceptions and return an emtpy value instead
- Added cargo config to set flag `NO_DYNAMIC_EXECUTION = 1` to avoid introducing the eval function

[0.1.1]: https://github.com/ethereumjs/rustbn.js/compare/v0.1.0...v0.1.1

## [0.1.0] - 2017-10-11
- Initial release

