<a name="0.7.5"></a>
## [0.7.5](https://github.com/multiformats/js-cid/compare/v0.7.4...v0.7.5) (2020-03-24)


### Features

* add nodejs.util.inspect.custom ([fe953c1](https://github.com/multiformats/js-cid/commit/fe953c1))



<a name="0.7.4"></a>
## [0.7.4](https://github.com/multiformats/js-cid/compare/v0.7.3...v0.7.4) (2020-03-16)


### Bug Fixes

* add buffer ([65681ef](https://github.com/multiformats/js-cid/commit/65681ef))



<a name="0.7.3"></a>
## [0.7.3](https://github.com/multiformats/js-cid/compare/v0.7.2...v0.7.3) (2020-01-24)


### Bug Fixes

* address review requests + ordering ([29e2def](https://github.com/multiformats/js-cid/commit/29e2def))
* update toString to include optional base ([5aff196](https://github.com/multiformats/js-cid/commit/5aff196))


### Features

* more correct type defs + docs ([4eb0c60](https://github.com/multiformats/js-cid/commit/4eb0c60))



<a name="0.7.2"></a>
## [0.7.2](https://github.com/multiformats/js-cid/compare/v0.7.1...v0.7.2) (2020-01-14)


### Bug Fixes

* codecs -> record of codec: buffer ([4cf17bb](https://github.com/multiformats/js-cid/commit/4cf17bb))
* explicitly require .json ext of base-table ([a9898ff](https://github.com/multiformats/js-cid/commit/a9898ff)), closes [#96](https://github.com/multiformats/js-cid/issues/96)


### Features

* typescript types ([3b27338](https://github.com/multiformats/js-cid/commit/3b27338))
* update types in package.json ([e01f19d](https://github.com/multiformats/js-cid/commit/e01f19d))



<a name="0.7.1"></a>
## [0.7.1](https://github.com/multiformats/js-cid/compare/v0.7.0...v0.7.1) (2019-05-14)


### Bug Fixes

* create new CID from old CID ([c888183](https://github.com/multiformats/js-cid/commit/c888183))



<a name="0.7.0"></a>
# [0.7.0](https://github.com/multiformats/js-cid/compare/v0.6.0...v0.7.0) (2019-05-09)


### Bug Fixes

* broken link to contributing document ([c29d12e](https://github.com/multiformats/js-cid/commit/c29d12e))
* update typedefs to reflect API changes ([63cd5f3](https://github.com/multiformats/js-cid/commit/63cd5f3)), closes [#77](https://github.com/multiformats/js-cid/issues/77)


### Code Refactoring

* default to base32 encoding for v1 CIDs ([2f854c7](https://github.com/multiformats/js-cid/commit/2f854c7))


### BREAKING CHANGES

* The default string encoding for v1 CIDs has changed from base58btc to base32.

License: MIT
Signed-off-by: Alan Shaw <alan.shaw@protocol.ai>



<a name="0.6.0"></a>
# [0.6.0](https://github.com/multiformats/js-cid/compare/v0.5.8...v0.6.0) (2019-04-08)


### Features

* add flow typedefs ([1cf9740](https://github.com/multiformats/js-cid/commit/1cf9740))
* cache string represntation ([537f604](https://github.com/multiformats/js-cid/commit/537f604))
* preserve base when constructed from a string ([2e597b9](https://github.com/multiformats/js-cid/commit/2e597b9))


### BREAKING CHANGES

* previously base was not preserved and all CIDs would
be normalised to base58btc when asking for their string representation.

The default will change to base32 in https://github.com/multiformats/js-cid/pull/73/files

The idea behind this change is that we shouldnt lose information when
the user passes us a base encoded string, but keep it and use it as
the default base so toString returns the same string they provided.

I'd like this as a fix for ipld explorer, which currently forces all
CIDs into base58btc, seee: https://github.com/ipfs-shipyard/ipfs-webui/issues/999

License: MIT
Signed-off-by: Oli Evans <oli@tableflip.io>



<a name="0.5.8"></a>
## [0.5.8](https://github.com/multiformats/js-cid/compare/v0.5.7...v0.5.8) (2019-03-14)


### Performance Improvements

* cache buffer form of CID when created ([c7fc646](https://github.com/multiformats/js-cid/commit/c7fc646))



<a name="0.5.7"></a>
## [0.5.7](https://github.com/multiformats/js-cid/compare/v0.5.6...v0.5.7) (2018-12-06)


### Bug Fixes

* stricter validation for CID v1 to v0 conversion ([0bd7318](https://github.com/multiformats/js-cid/commit/0bd7318))



<a name="0.5.6"></a>
## [0.5.6](https://github.com/ipld/js-cid/compare/v0.5.5...v0.5.6) (2018-11-22)


### Bug Fixes

* add class name ([b9fc845](https://github.com/ipld/js-cid/commit/b9fc845))
* generated docs, re-add isCID ([5b826fc](https://github.com/ipld/js-cid/commit/5b826fc))
* **package:** update multibase to version 0.6.0 ([e4e6508](https://github.com/ipld/js-cid/commit/e4e6508))



<a name="0.5.5"></a>
## [0.5.5](https://github.com/ipld/js-cid/compare/v0.5.4...v0.5.5) (2018-09-25)


### Bug Fixes

* toV0 and toV1 create instances that cause isCID be false ([14ab8e4](https://github.com/ipld/js-cid/commit/14ab8e4))



<a name="0.5.4"></a>
## [0.5.4](https://github.com/ipld/js-cid/compare/v0.5.3...v0.5.4) (2018-09-24)


### Bug Fixes

* linter errors ([9f9359d](https://github.com/ipld/js-cid/commit/9f9359d))
* migrate to class-is for instance comparise, fixes [#53](https://github.com/ipld/js-cid/issues/53) ([6b6873b](https://github.com/ipld/js-cid/commit/6b6873b))
* remove direct access to codec lookup table ([4027108](https://github.com/ipld/js-cid/commit/4027108))
* use org/repo convention ([5805660](https://github.com/ipld/js-cid/commit/5805660))


### Features

* add toString function ([f47e68c](https://github.com/ipld/js-cid/commit/f47e68c))



<a name="0.5.3"></a>
## [0.5.3](https://github.com/ipld/js-cid/compare/v0.5.2...v0.5.3) (2018-03-12)


### Bug Fixes

* [#39](https://github.com/ipld/js-cid/issues/39) - Improve constructor performance ([#45](https://github.com/ipld/js-cid/issues/45)) ([dc0bfd3](https://github.com/ipld/js-cid/commit/dc0bfd3))



<a name="0.5.2"></a>
## [0.5.2](https://github.com/ipld/js-cid/compare/v0.5.1...v0.5.2) (2017-10-06)



<a name="0.5.1"></a>
## [0.5.1](https://github.com/ipld/js-cid/compare/v0.5.0...v0.5.1) (2017-07-10)



<a name="0.5.0"></a>
# [0.5.0](https://github.com/ipld/js-cid/compare/v0.4.2...v0.5.0) (2017-03-30)


### Bug Fixes

* add CID validation ([#30](https://github.com/ipld/js-cid/issues/30)) ([38e5dd0](https://github.com/ipld/js-cid/commit/38e5dd0))



<a name="0.4.2"></a>
## [0.4.2](https://github.com/ipld/js-cid/compare/v0.4.1...v0.4.2) (2017-03-16)


### Bug Fixes

* **package:** update multihashes to version 0.4.0 ([1d0c3c8](https://github.com/ipld/js-cid/commit/1d0c3c8))



<a name="0.4.1"></a>
## [0.4.1](https://github.com/ipld/js-cid/compare/v0.4.0...v0.4.1) (2017-02-09)



<a name="0.4.0"></a>
# [0.4.0](https://github.com/ipld/js-cid/compare/v0.3.6...v0.4.0) (2017-01-25)


### Bug Fixes

* make toV0 and toV1 return CID objects ([32902e3](https://github.com/ipld/js-cid/commit/32902e3))
* throw an error if another base is picked for cidv0 ([24f2c0b](https://github.com/ipld/js-cid/commit/24f2c0b))



<a name="0.3.6"></a>
## [0.3.6](https://github.com/ipld/js-cid/compare/v0.3.5...v0.3.6) (2017-01-21)



<a name="0.3.5"></a>
## [0.3.5](https://github.com/ipld/js-cid/compare/v0.3.4...v0.3.5) (2016-12-16)


### Features

* add prefix feature ([7dae38e](https://github.com/ipld/js-cid/commit/7dae38e))



<a name="0.3.4"></a>
## [0.3.4](https://github.com/ipfs/js-cid/compare/v0.3.3...v0.3.4) (2016-12-08)



<a name="0.3.3"></a>
## [0.3.3](https://github.com/ipfs/js-cid/compare/v0.3.2...v0.3.3) (2016-12-07)



<a name="0.3.2"></a>
## [0.3.2](https://github.com/ipfs/js-cid/compare/v0.3.1...v0.3.2) (2016-12-07)



<a name="0.3.1"></a>
## [0.3.1](https://github.com/ipfs/js-cid/compare/v0.3.0...v0.3.1) (2016-12-07)


### Bug Fixes

* dependencies ([54f29f9](https://github.com/ipfs/js-cid/commit/54f29f9))



<a name="0.3.0"></a>
# [0.3.0](https://github.com/ipfs/js-cid/compare/v0.2.0...v0.3.0) (2016-12-05)


### Features

* **deps:** update to multihashes[@0](https://github.com/0).3.0 ([a0e331d](https://github.com/ipfs/js-cid/commit/a0e331d))



<a name="0.2.0"></a>
# [0.2.0](https://github.com/ipfs/js-cid/compare/v0.1.1...v0.2.0) (2016-10-24)


### Features

* cidV0 and cidV1 support ([211970b](https://github.com/ipfs/js-cid/commit/211970b))



<a name="0.1.1"></a>
## 0.1.1 (2016-09-09)



