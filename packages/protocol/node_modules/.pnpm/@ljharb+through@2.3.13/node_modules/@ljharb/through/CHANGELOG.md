# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [v2.3.13](https://github.com/ljharb/through/compare/v2.3.12...v2.3.13) - 2024-03-08

### Commits

- [types] use shared tsconfig [`8e77c7b`](https://github.com/ljharb/through/commit/8e77c7bd23365d0950730fa4e402c17549b48680)
- [Tests] increase coverage [`0a0a33f`](https://github.com/ljharb/through/commit/0a0a33fb2a5fbe63d449371f1e3ea91a969b8817)
- [types] fix d.ts errors; use a namespace [`653e55d`](https://github.com/ljharb/through/commit/653e55d7bc2ac4f43500e327e45bb2b8c24e4a1f)
- [actions] remove redundant finisher; use reusable workflows [`65ab32c`](https://github.com/ljharb/through/commit/65ab32c807b748cd96623169b99561aa05993d6d)
- [Dev Deps] update `@arethetypeswrong/cli`, `tape`, `typescript` [`1d2c513`](https://github.com/ljharb/through/commit/1d2c51309a4b5d70aa16431c9514b2805d7374ec)
- [Dev Deps] update `@types/node`, `typescript` [`d5bdc23`](https://github.com/ljharb/through/commit/d5bdc23afc0610c37524b4ef615fd3aee442b16d)
- [Deps] update `call-bind` [`2633dc6`](https://github.com/ljharb/through/commit/2633dc6c9c728099cf105c40ad8990c463fbc054)
- [Tests] install missing `@types/node` [`fe34373`](https://github.com/ljharb/through/commit/fe343735e8df0122d1856fc4f88c46e3ee1a33df)

## [v2.3.12](https://github.com/ljharb/through/compare/v2.3.11...v2.3.12) - 2024-01-24

### Commits

- [patch] add TS types [`0b68b9d`](https://github.com/ljharb/through/commit/0b68b9dc1655edbf5dc81cccdb3ef23dcd8e4bbd)
- [readme] correct requires; remove redundant license section [`90ff780`](https://github.com/ljharb/through/commit/90ff780f9c3f2c2fb345b33c87e0be080dbf7950)
- [Dev Deps] update `aud`, `npmignore`, `tape` [`f4456ce`](https://github.com/ljharb/through/commit/f4456ceaf2b216858e7652c9eef8d96badbad678)
- [Deps] update `call-bind` [`c1b7daa`](https://github.com/ljharb/through/commit/c1b7daaf9ea8138d90caf7cfe7250a09760034c2)
- [Dev Deps] update `tape` [`74a75ac`](https://github.com/ljharb/through/commit/74a75acadcca26c71ec15f0f0d48e3a9265cebf3)
- [meta] add `sideEffects` flag [`dcfef36`](https://github.com/ljharb/through/commit/dcfef3615e7df63219aa6184469f4b2a1e61cb0b)

## [v2.3.11](https://github.com/ljharb/through/compare/v2.3.10...v2.3.11) - 2023-10-11

### Commits

- [Deps] add missing `call-bind` [`da5e46d`](https://github.com/ljharb/through/commit/da5e46d31045e18b79dc137bbc2bde1ebf839b38)

## [v2.3.10](https://github.com/ljharb/through/compare/v2.3.9...v2.3.10) - 2023-10-10

### Commits

- [Robustness] use `call-bind` [`3fc8de1`](https://github.com/ljharb/through/commit/3fc8de1dd6b5c69ebca1b212d4852ca11e853cb2)
- [Dev Deps] update `aud`, `tape` [`db14fcf`](https://github.com/ljharb/through/commit/db14fcf1457d1e35b22856de42767d0bf4773fd8)

## [v2.3.9](https://github.com/ljharb/through/compare/v2.3.8...v2.3.9) - 2023-07-17

### Commits

- [meta] add scripts, dev deps, linting; switch from travis to GHA [`2bc7e2d`](https://github.com/ljharb/through/commit/2bc7e2d1f63caf9cbdd146c9d752d3f62f564405)
- [meta] rename package to scoped [`337610b`](https://github.com/ljharb/through/commit/337610b937196ad26abe40c7d191942c0d54f455)
- [meta] confirmed with dominictarr in writing that this package was only ever MIT [`832061e`](https://github.com/ljharb/through/commit/832061e15beb9643b74e16b1f8343c36620f5be7)
- [Dev Deps] update `from`, `stream-spec`, `tape` [`dae9dda`](https://github.com/ljharb/through/commit/dae9dda68c7df31960239957e7694de1f3bca40a)
- [meta] fix auto-changelog start version [`3ac451b`](https://github.com/ljharb/through/commit/3ac451b684d4b4cb9356eb758691e6438b0083d1)
- [Fix] `node &lt; v0.6` does not have `stream` === `stream.Stream` [`9f72162`](https://github.com/ljharb/through/commit/9f721627f62dfa4dad1f5515f4d3d1aa3613ccb0)

<!-- auto-changelog-above -->

## [v2.3.8](https://github.com/ljharb/through/compare/v2.3.7...v2.3.8) - 2015-07-03

### Merged

- Update package.json [`#35`](https://github.com/dominictarr/through/pull/35)

## [v2.3.7](https://github.com/ljharb/through/compare/v2.3.6...v2.3.7) - 2015-04-06

### Commits

- fix non-strict null check [`7048350`](https://github.com/dominictarr/through/commit/7048350a5dae8bbbb3b9a972ecec4fe2460d113d)

## [v2.3.6](https://github.com/ljharb/through/compare/v2.3.5...v2.3.6) - 2014-09-16

### Commits

- simple async test [`a98561f`](https://github.com/dominictarr/through/commit/a98561fae17615da8fe6c9dc00208c5107bf9774)
- update test to work in the browser (without process.on("exit") [`7dabce0`](https://github.com/dominictarr/through/commit/7dabce06b4d367f280846a72fe468ace54d55456)
- remove concat-stream [`69d31f3`](https://github.com/dominictarr/through/commit/69d31f3efe1600a44787cb198d7b97ae7eaa7167)
- add dev deps [`d387710`](https://github.com/dominictarr/through/commit/d387710cccaeec66dc889377778c17254ea13ef4)
- validate before end [`c267d90`](https://github.com/dominictarr/through/commit/c267d90e3cdb501510edfd590245fd1d58fe29bd)
- use tape@2.3.2 [`1080f0c`](https://github.com/dominictarr/through/commit/1080f0ca8d32813cd50be9eac5975504fb5b8a55)
- testling badge [`aa23057`](https://github.com/dominictarr/through/commit/aa23057876709666ab4d02f0cd7a36797de77c80)
- tidy [`c5d9757`](https://github.com/dominictarr/through/commit/c5d9757c8c8e31d563dc4a4bbb02a5f7e6304360)

## [v2.3.5](https://github.com/ljharb/through/compare/v2.3.4...v2.3.5) - 2013-05-05

### Fixed

- fix spelling Closes #13 [`#13`](https://github.com/dominictarr/through/issues/13)

## [v2.3.4](https://github.com/ljharb/through/compare/v2.3.3...v2.3.4) - 2013-04-29

### Merged

- mark "write" method as code [`#14`](https://github.com/dominictarr/through/pull/14)

### Commits

- fix typo [`e94d772`](https://github.com/dominictarr/through/commit/e94d7726f765e4fcb83169ba845ebbb9fec418c7)

## [v2.3.3](https://github.com/ljharb/through/compare/v2.3.2...v2.3.3) - 2013-04-24

### Merged

- Test on 0.10 on Travis [`#12`](https://github.com/dominictarr/through/pull/12)

## [v2.3.2](https://github.com/ljharb/through/compare/v2.3.1...v2.3.2) - 2013-04-24

### Commits

- check that through ends exactly once [`802ecff`](https://github.com/dominictarr/through/commit/802ecff64d8462be9e1db6a578857393a37b19eb)
- emit "end" exactly once [`6f814a6`](https://github.com/dominictarr/through/commit/6f814a601b37db1f44113e56a1eaa0c32a33b0a2)

## [v2.3.1](https://github.com/ljharb/through/compare/v2.3.0...v2.3.1) - 2013-04-13

### Commits

- syntax error [`b90e6ba`](https://github.com/dominictarr/through/commit/b90e6ba08b626899f8007367d2a934714a9cdc23)

## [v2.3.0](https://github.com/ljharb/through/compare/v2.2.7...v2.3.0) - 2013-04-13

### Commits

- test autoDestroy option [`a2e1d5d`](https://github.com/dominictarr/through/commit/a2e1d5dad7f5ace3a174145ab32d58642bc1d685)
- document autoDestroy [`c4520e5`](https://github.com/dominictarr/through/commit/c4520e59f6a96f12eebfadef6a746d266b481d25)
- implement autoDestroy option [`1cabed2`](https://github.com/dominictarr/through/commit/1cabed2d3a57bec745d6a5c90d0c3077254dadf9)

## [v2.2.7](https://github.com/ljharb/through/compare/v2.2.6...v2.2.7) - 2013-03-15

### Merged

- Typo fix [`#10`](https://github.com/dominictarr/through/pull/10)

### Commits

- wider range [`e685bc2`](https://github.com/dominictarr/through/commit/e685bc2299f28d91c5a9d3eac7d929fdcff5767f)

## [v2.2.6](https://github.com/ljharb/through/compare/v2.2.5...v2.2.6) - 2013-03-13

### Commits

- update stream-spec [`1483cd4`](https://github.com/dominictarr/through/commit/1483cd44dbcd089c3bbb9e0111c09a64d0ecb715)

## [v2.2.5](https://github.com/ljharb/through/compare/v2.2.4...v2.2.5) - 2013-03-13

### Commits

- hopefully make this work in browserify [`a346472`](https://github.com/dominictarr/through/commit/a3464727990c9b273cec1b778b59aabb7bbc77c8)
- narrow testling range, so it does not take as long [`b1e474b`](https://github.com/dominictarr/through/commit/b1e474b029827673de4b35850d2116e7176171bf)

## [v2.2.4](https://github.com/ljharb/through/compare/v2.2.3...v2.2.4) - 2013-03-13

### Commits

- update stream spec [`f0f0c83`](https://github.com/dominictarr/through/commit/f0f0c83cd32a31dcf897b4eb310210440e975d89)

## [v2.2.3](https://github.com/ljharb/through/compare/v2.2.2...v2.2.3) - 2013-03-12

### Commits

- update stream-spec [`2d2e3e4`](https://github.com/dominictarr/through/commit/2d2e3e4ab429cd9c1a9a9d6b49488040012b0d33)

## [v2.2.2](https://github.com/ljharb/through/compare/v2.2.1...v2.2.2) - 2013-03-12

### Commits

- switch tests to use tape [`4a5652f`](https://github.com/dominictarr/through/commit/4a5652f8ada85dd8d212d558f386e2ab57f36d46)
- add testling setup to package.json [`bbe2a05`](https://github.com/dominictarr/through/commit/bbe2a05012b4855624893c24ebed1c24fb3d8107)
- fix test command [`12ec8a3`](https://github.com/dominictarr/through/commit/12ec8a34f6155ca7ea504aa1a0eaa30a869c90bc)

## [v2.2.1](https://github.com/ljharb/through/compare/2.2.0...v2.2.1) - 2013-03-03

### Merged

- doc [`#6`](https://github.com/dominictarr/through/pull/6)

## [2.2.0](https://github.com/ljharb/through/compare/v2.1.0...2.2.0) - 2013-02-11

### Merged

- Alias `queue()` to `push()` [`#5`](https://github.com/dominictarr/through/pull/5)

## [v2.1.0](https://github.com/ljharb/through/compare/v2.0.0...v2.1.0) - 2012-11-27

### Commits

- allow chaining [`6fbfd15`](https://github.com/dominictarr/through/commit/6fbfd158bc64a0df1b9cb8e6e462ab8109caaf09)

## [v2.0.0](https://github.com/ljharb/through/compare/v1.1.2...v2.0.0) - 2012-11-25

### Commits

- explain new api [`f5c1702`](https://github.com/dominictarr/through/commit/f5c17027e88d0cb8f51ba02dff058f909064617a)
- internal buffer should not be exposed [`97b40cc`](https://github.com/dominictarr/through/commit/97b40ccb4fe3058ad6d2d625c6d03297991a551a)
- use queue by default [`105677a`](https://github.com/dominictarr/through/commit/105677a705621f69ac5c2e42dc7f378b62eef937)

## [v1.1.2](https://github.com/ljharb/through/compare/v1.1.1...v1.1.2) - 2012-11-20

### Commits

- Ensure that end will be queued if paused with data [`c3efc00`](https://github.com/dominictarr/through/commit/c3efc00a7f381901096f0b3704aff2e2e3585330)

## [v1.1.1](https://github.com/ljharb/through/compare/v1.1.0...v1.1.1) - 2012-10-11

### Commits

- tidy [`42a2914`](https://github.com/dominictarr/through/commit/42a2914644a52981a9dbc94edbfe2d4677763817)

## [v1.1.0](https://github.com/ljharb/through/compare/v0.1.4...v1.1.0) - 2012-09-15

### Commits

- optional buffering [`63b41c6`](https://github.com/dominictarr/through/commit/63b41c6e073998cd8ed79a729339b1a728be088d)
- test for buffering [`9038d80`](https://github.com/dominictarr/through/commit/9038d8062ce4385fa34198d663597fae2b60b86d)
- test that end, close event correct [`7438a33`](https://github.com/dominictarr/through/commit/7438a333b3ab43121f654c26230fe0f39e3a33b1)
- update docs [`5f55881`](https://github.com/dominictarr/through/commit/5f5588176395d5934287205a599c0789150bc209)
- always emit drain [`58ce6ed`](https://github.com/dominictarr/through/commit/58ce6ed1fce57a777e8031384f1fa75ce185ec5d)

## [v0.1.4](https://github.com/ljharb/through/compare/v0.1.3...v0.1.4) - 2012-08-07

### Commits

- package.json: s/repo/repository/ [`6c811fa`](https://github.com/dominictarr/through/commit/6c811fa1c8d90b54f3d41e969720fb405f1de6c1)

## [v0.1.3](https://github.com/ljharb/through/compare/v0.1.2...v0.1.3) - 2012-08-07

### Commits

- fix repo links [`c49724d`](https://github.com/dominictarr/through/commit/c49724d51a4ca6bd3096451f98a7cd5b45b4386f)

## [v0.1.2](https://github.com/ljharb/through/compare/v0.1.1...v0.1.2) - 2012-07-23

### Commits

- do not emit pause event when already paused [`1e5a27e`](https://github.com/dominictarr/through/commit/1e5a27e2c49a82e769f4364126f7315894e81021)

## [v0.1.1](https://github.com/ljharb/through/compare/v0.1.0...v0.1.1) - 2012-07-23

### Commits

- fix typo [`3258738`](https://github.com/dominictarr/through/commit/325873812e1893472ab141b69fe48665cb5b7ac3)

## [v0.1.0](https://github.com/ljharb/through/compare/v0.0.4...v0.1.0) - 2012-07-23

### Commits

- emit pause on entering pause state [`2a003f9`](https://github.com/dominictarr/through/commit/2a003f9896b19978f6111e077acab870e5c9bbf6)

## [v0.0.4](https://github.com/ljharb/through/compare/v0.0.3...v0.0.4) - 2012-07-07

### Commits

- add opensource [`9691145`](https://github.com/dominictarr/through/commit/9691145e2791855294258ab02bbbdbb132a23c43)
- a bit better test coverage [`320ee05`](https://github.com/dominictarr/through/commit/320ee05d28b9aa7f0242446b803217947ad8d25f)
- do not throw on end if not writable, just ignore [`a04701b`](https://github.com/dominictarr/through/commit/a04701be062484c02d54b6515583b31930bed1f9)

## [v0.0.3](https://github.com/ljharb/through/compare/v0.0.2...v0.0.3) - 2012-07-05

### Commits

- remove from [`682c71e`](https://github.com/dominictarr/through/commit/682c71e0774f6442d7fc64ee4bc9b6a76fead177)
- add travis-ci [`8ccc78c`](https://github.com/dominictarr/through/commit/8ccc78c66989845e4f538fe9a041ebfe9f52f672)
- don't care about 0.4 [`df41950`](https://github.com/dominictarr/through/commit/df419507a80c30586ada6576780228cc39c5fbda)

## [v0.0.2](https://github.com/ljharb/through/compare/v0.0.1...v0.0.2) - 2012-07-05

### Commits

- document [`b238767`](https://github.com/dominictarr/through/commit/b2387678b54fdb08093eb3e23b54f3e1c8477237)

## v0.0.1 - 2012-07-05

### Commits

- initial [`903501a`](https://github.com/dominictarr/through/commit/903501acf73d03f584dd572b885c78c1bc9cca8b)
- add test script [`7359f4d`](https://github.com/dominictarr/through/commit/7359f4dfe0df20938f77bdbdf689eba78b8a0fa4)
