# Change Log

All notable changes to this project will be documented in this file.
See [Conventional Commits](https://conventionalcommits.org) for commit guidelines.

## [0.9.4](http://chainsafe/ssz/compare/@chainsafe/ssz@0.9.3...@chainsafe/ssz@0.9.4) (2022-12-08)

**Note:** Version bump only for package @chainsafe/ssz





## [0.9.3](http://chainsafe/ssz/compare/@chainsafe/ssz@0.9.2...@chainsafe/ssz@0.9.3) (2022-12-08)

**Note:** Version bump only for package @chainsafe/ssz





## [0.9.2](https://github.com/chainsafe/ssz/compare/@chainsafe/ssz@0.9.1...@chainsafe/ssz@0.9.2) (2022-05-31)

* Fix ListCompositeType.sliceTo(-1) (#268)

## [0.9.1](https://github.com/chainsafe/ssz/compare/@chainsafe/ssz@0.9.0...@chainsafe/ssz@0.9.1) (2022-04-14)

* Force usage of Uint8Array.prototype.slice (#258)
* Add and use a new helper to digest64 two 32 bytes (#255)
* Remove unused files (#248)
* Bump spec tests (#251)
* Bump yargs-parser from 16.1.0 to 20.2.4 in /packages/ssz (#189)
* Test empty ByteListType (#250)

# [0.9.0](http://chainsafe/ssz/compare/@chainsafe/ssz@0.8.19...@chainsafe/ssz@0.9.0) (2022-03-24)


* SSZ v2 (#223) ([9d167b7](http://chainsafe/ssz/commits/9d167b703b1e974ee4943be15710aa9783183986)), closes [#223](http://chainsafe/ssz/issues/223) [#227](http://chainsafe/ssz/issues/227)
* Convert as-sha256 to typescript (#244) ([2d4e3fe](http://chainsafe/ssz/commits/2d4e3febec89ca8ca7c89a19c6949c3213c2c45c)), closes [#244](http://chainsafe/ssz/issues/244)


### BREAKING CHANGES

* complete refactor, see packages/ssz/README.md for details

## 0.8.20 (2021-11-23)
- Harden ssz implementation [#211](https://github.com/ChainSafe/ssz/pull/211)

## [0.8.19](https://github.com/chainsafe/ssz/compare/@chainsafe/ssz@0.8.18...@chainsafe/ssz@0.8.19) (2021-10-12)

**Note:** Version bump only for package @chainsafe/ssz

## 0.8.18 (2021-09-25)

## Features

- Ability to specify casingMap declaration time for Container's toJson/fromJson [#198](https://github.com/ChainSafe/ssz/pull/198)
- Extending case matching in Container's toJson/fromJson with a range of case types [#184](https://github.com/ChainSafe/ssz/pull/184)
- Ability to provide casingMap in toJson/fromJson interface for aiding case matching [#184](https://github.com/ChainSafe/ssz/pull/184)


## 0.8.17 (2021-08-30)

## Bug fixes

- Fix ContainerLeafNodeStructTreeValue [#179](https://github.com/ChainSafe/ssz/pull/179)

## 0.8.16 (2021-08-30)

## Features

- Implement Number64UintType and Number64ListType [#159](https://github.com/ChainSafe/ssz/pull/159)
- Add ContainerLeafNodeStructType for memory efficiency [#168](https://github.com/ChainSafe/ssz/pull/168)
- Union type [#145](https://github.com/ChainSafe/ssz/pull/145)
- Add alternative to iterator interface [#171](https://github.com/ChainSafe/ssz/pull/171)

## 0.8.15 (2021-08-23)

## Features

- Avoid iterateNodesAtDepth [#167](https://github.com/ChainSafe/ssz/pull/167)

## 0.8.14 (2021-08-19)

## Features

- Add getFixedSerializedLength() [#148](https://github.com/ChainSafe/ssz/pull/148)
- Cache field information for Container type [#146](https://github.com/ChainSafe/ssz/pull/146)
- Use persistent-merkle-tree 0.3.5 and as-sha256 0.2.4 [#165](https://github.com/ChainSafe/ssz/pull/165)

## 0.8.13 (2021-08-04)

## Features

- Use utility function for bigint exponentiation ([c2d3a7](https://github.com/chainsafe/ssz/commit/c2d3a7))

## 0.8.12 (2021-07-30)

## Features

- Use utility function for bigint exponentiation ([fbb671](https://github.com/chainsafe/ssz/commit/fbb671))

## 0.8.11 (2021-06-18)

## Chores

- Update persistent-merkle-tree ([f88f15](https://github.com/chainsafe/ssz/commit/f88f15))

## 0.8.10 (2021-06-16)

## Features

- Add fast struct_equals impl for RootType ([a3f4b4](https://github.com/chainsafe/ssz/commit/a3f4b4))

## 0.8.9 (2021-06-11)

## Bug Fixes

- Fix tree_serializeToBytes offset in BitList ([7cd2c1](https://github.com/chainsafe/ssz/commit/7cd2c1))

## 0.8.8 (2021-06-10)

## Bug Fixes

- Fix tree serialize/deserialize in BitList ([ee47a0](https://github.com/chainsafe/ssz/commit/ee47a0))
- Fix struct_getRootAtChunkIndex for BitVectorType ([ee47a0](https://github.com/chainsafe/ssz/commit/ee47a0))

## 0.8.7 (2021-06-01)

## Bug Fixes

- Fix BitVector tree_deserializeFromBytes() ([fd2c7d](https://github.com/chainsafe/ssz/commit/fd2c7d))

## 0.8.6 (2021-05-25)

## Bug Fixes

- Fix basic vector struct_convertFromJson ([05e7c2](https://github.com/chainsafe/ssz/commit/05e7c2))

## 0.8.5 (2021-05-19)

## Features

- Add composite type leaves to proof ([3f1cfd](https://github.com/chainsafe/ssz/commit/3f1cfd))

## 0.8.4 (2021-05-07)

## Bug Fixes

- Update as-sha256 ([8d497b](https://github.com/chainsafe/ssz/commit/8d497b))

## 0.8.3 (2021-05-04)

## Features

- Improve hexToString performance ([106991](https://github.com/chainsafe/ssz/commit/106991))

## Chores

- Update as-sha256 & persistent-merkle-tree ([212927](https://github.com/chainsafe/ssz/commit/212927))
- Use for of instead of forEach ([e195df](https://github.com/chainsafe/ssz/commit/e195df))
- Add whitespace ([516421](https://github.com/chainsafe/ssz/commit/516421))

## 0.8.2 (2021-04-05)

## Features

- Add tree_createFromProof ([b804f4](https://github.com/chainsafe/ssz/commit/b804f4))

## 0.8.1 (2021-04-02)

## Bug Fixes

- Fix bit array struct->tree ([8b08ea](https://github.com/chainsafe/ssz/commit/8b08ea))

## Features

- Improve convertToTree ([776b63](https://github.com/chainsafe/ssz/commit/776b63))

## 0.8.0 (2021-03-29)

## BREAKING CHANGES

- Refactor codebase ([c86871](https://github.com/chainsafe/ssz/commit/c86871))

## 0.7.1 (2021-03-29)

## Bug Fixes

- Bug fix in @chainsafe/persistent-merkle-tree ([d31f82](https://github.com/chainsafe/ssz/commit/d31f82))

## 0.7.0 (2021-03-01)

## BREAKING CHANGES

- Use abstract class Type/BasicType/CompositeType ([c91b19](https://github.com/chainsafe/ssz/commit/c91b19))
- Remove TreeBacked<T>#gindexOfProperty ([d4b141](https://github.com/chainsafe/ssz/commit/d4b141))

## Features

- Optimize byte array equals ([7639b2](https://github.com/chainsafe/ssz/commit/7639b2))
- Restrict the use of BigInt whenever possible ([22e8c4](https://github.com/chainsafe/ssz/commit/22e8c4))
- Initial multiproof support ([9e758a](https://github.com/chainsafe/ssz/commit/9e758a))

## 0.6.13 (2020-08-05)

## Bug Fixes

- Add length check to array structural equality ([c7782a](https://github.com/chainsafe/ssz/commit/c7782a))

## 0.6.12 (2020-08-05)

## Features

- Add readOnlyEntries ([211e6d](https://github.com/chainsafe/ssz/commit/211e6d))

## 0.6.11 (2020-08-01)

## Features

- Optimize fromStructural ([ff388a](https://github.com/chainsafe/ssz/commit/ff388a))

## 0.6.10 (2020-07-27)

## Features

- Add readOnlyForEach and readOnlyMap functions ([356d70](https://github.com/chainsafe/ssz/commit/356d70))

## Chores

- Use readonly tree iteration in tree-backed toBytes ([356d70](https://github.com/chainsafe/ssz/commit/356d70))

## 0.6.9 (2020-07-09)

### Bug Fixes

- Fix bitlist/bitvector validation ([030784](https://github.com/chainsafe/ssz/commit/030784))

## 0.6.8 (2020-07-09)

### Features

- Track error JSON path location with try / catch ([4ff92d](https://github.com/chainsafe/ssz/commit/4ff92d))
- Add validation for fromBytes ([64b757](https://github.com/chainsafe/ssz/commit/64b757))

### Chores

- Add prettier as an eslint plugin ([c606a0](https://github.com/chainsafe/ssz/commit/c606a0))
- Add es and node badges ([b25c39](https://github.com/chainsafe/ssz/commit/b25c39))
- Standardize uint toJson ([4dbc6f](https://github.com/chainsafe/ssz/commit/4dbc6f))

### Bug Fixes

- Fix structural bitlist/bitvector hashtreeroot ([5ed8e0](https://github.com/chainsafe/ssz/commit/5ed8e0))
- Fix start param validation case ([5fd207](https://github.com/chainsafe/ssz/commit/5fd207))

## 0.6.7 (2020-06-08)

### Features

- Implement minSize and mazSize api ([fb14bd](https://github.com/chainsafe/ssz/commit/fb14bd))

### Chores

- Don't use BigInt in NumberUintType ([7abe06](https://github.com/chainsafe/ssz/commit/7abe06))

## 0.6.6 (2020-06-07)

### Chores

- Update persistent-merkle-tree dependency ([b54ea0](https://github.com/chainsafe/ssz/commit/b54ea0))

## 0.6.5 (2020-06-01)

### Bug Fixes

- Fix Infinity serialization to/fromJson ([9a68c3](https://github.com/chainsafe/ssz/commit/9a68c3))

## 0.6.4 (2020-05-04)

### Features

- Add ObjBacked wrapper type ([1cf4ac](https://github.com/chainsafe/ssz/commit/1cf4ac))

## 0.6.3 (2020-04-30)

### Features

- Add isFooType functions ([a577e3](https://github.com/chainsafe/ssz/commit/a577e3))
- Add json case option ([0f8566](https://github.com/chainsafe/ssz/commit/0f8566))

### Bug Fixes

- Fix far future deserialization ([63d6cd](https://github.com/chainsafe/ssz/commit/63d6cd))

## 0.6.2 (2020-04-20)

### Chores

- Update persistent-merkle-tree dependency ([e05265](https://github.com/chainsafe/ssz/commit/e05265))

### Bug Fixes

- Fix composite vector defaultValue ([facf47](https://github.com/chainsafe/ssz/commit/facf47))
