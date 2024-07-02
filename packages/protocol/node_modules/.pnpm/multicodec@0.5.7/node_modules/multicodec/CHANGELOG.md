<a name="0.5.7"></a>
## [0.5.7](https://github.com/multiformats/js-multicodec/compare/v0.5.6...v0.5.7) (2019-12-22)


### Bug Fixes

* name table not hex encoded ([88bf273](https://github.com/multiformats/js-multicodec/commit/88bf273))
* remove unused table file ([f3766e5](https://github.com/multiformats/js-multicodec/commit/f3766e5))



<a name="0.5.6"></a>
## [0.5.6](https://github.com/multiformats/js-multicodec/compare/v0.5.5...v0.5.6) (2019-12-05)


### Features

* reduce bundle size by 1/3 ([#49](https://github.com/multiformats/js-multicodec/issues/49)) ([2422b09](https://github.com/multiformats/js-multicodec/commit/2422b09))



<a name="0.5.4"></a>
## [0.5.4](https://github.com/multiformats/js-multicodec/compare/v0.5.3...v0.5.4) (2019-07-16)


### Features

* add ipns-ns ([#46](https://github.com/multiformats/js-multicodec/issues/46)) ([efc5aa6](https://github.com/multiformats/js-multicodec/commit/efc5aa6))



<a name="0.5.3"></a>
## [0.5.3](https://github.com/multiformats/js-multicodec/compare/v0.5.2...v0.5.3) (2019-05-28)



<a name="0.5.2"></a>
## [0.5.2](https://github.com/multiformats/js-multicodec/compare/v0.5.1...v0.5.2) (2019-05-28)


### Features

* add libp2p-key ([db71d5c](https://github.com/multiformats/js-multicodec/commit/db71d5c))
* add method to convert codec names to numbers and back ([a18bce9](https://github.com/multiformats/js-multicodec/commit/a18bce9))



<a name="0.5.1"></a>
## [0.5.1](https://github.com/multiformats/js-multicodec/compare/v0.5.0...v0.5.1) (2019-04-19)


### Bug Fixes

* update the multicodec table ([2e5367f](https://github.com/multiformats/js-multicodec/commit/2e5367f)), closes [#40](https://github.com/multiformats/js-multicodec/issues/40)



<a name="0.5.0"></a>
# [0.5.0](https://github.com/multiformats/js-multicodec/compare/v0.4.0...v0.5.0) (2019-01-24)


### Features

* add codes as constants ([1c43120](https://github.com/multiformats/js-multicodec/commit/1c43120))
* add convenience function for getting the codec code ([ab110e3](https://github.com/multiformats/js-multicodec/commit/ab110e3))
* update scripts ([b74f269](https://github.com/multiformats/js-multicodec/commit/b74f269))



<a name="0.4.0"></a>
# [0.4.0](https://github.com/multiformats/js-multicodec/compare/v0.3.0...v0.4.0) (2019-01-08)


* Revert "feat: make adding a custom codec possible" ([9e251ce](https://github.com/multiformats/js-multicodec/commit/9e251ce))


### Chores

* fully automatic table generation ([f3d8c0d](https://github.com/multiformats/js-multicodec/commit/f3d8c0d))


### BREAKING CHANGES

* multibase is not part of this package anymore

As multibase works differently from multicodec, those codecs
were [removed from the multicodec table], hence those are also
removed from this implementation as we have automatic
conversion from the upstream table.

[removed from the multicodec table]:
https://github.com/multiformats/multicodec/commit/1ec0e971d589d2fd5d5418b212846301909525bd
* the `addCodec()` function is removed

The `addCodec()` function is removed as it doesn't work as expected.
Things break as soon as the module is loaded several times, which
can happen if dependencies require a different version.

Steps to reproduce this problem:

```console
$ mkdir addcodecbug
$ cd addcodecbug
$ npm install cids@0.5.7 multicodec@0.3.0
npm WARN saveError ENOENT: no such file or directory, open '/tmp/addcodecbug/package.json'
npm notice created a lockfile as package-lock.json. You should commit this file.
npm WARN enoent ENOENT: no such file or directory, open '/tmp/addcodecbug/package.json'
npm WARN addcodecbug No description
npm WARN addcodecbug No repository field.
npm WARN addcodecbug No README data
npm WARN addcodecbug No license field.

+ multicodec@0.3.0
+ cids@0.5.7
added 10 packages from 35 contributors and audited 14 packages in 1.363s
found 0 vulnerabilities
$ cat > index.js <<'EOF'
// Uses multicodec v0.2.7
const CID = require('cids')
// Imports multicodec v0.3.0
const multicodec = require('multicodec')

multicodec.addCodec('my-codec', Buffer.from('5566', 'hex'))
// Works, the codec was added
console.log(multicodec.getCodeVarint('my-codec'))

const multihash = Buffer.from(
  '1220b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9',
  'hex')
const cid = new CID(1, 'my-codec', multihash)
console.log(cid)
// Fails as `my-codec` was only added to the multicodec module loaded by this
// file, and not the one loaded by `cids`.
console.log(cid.toBaseEncodedString())
EOF
$ node index.js
<Buffer e6 aa 01>
CID {
  codec: 'my-codec',
  version: 1,
  multihash:
   <Buffer 12 20 b9 4d 27 b9 93 4d 3e 08 a5 2e 52 d7 da 7d ab fa c4 84 ef e3 7a 53 80 ee 90 88 f7 ac e2 ef cd e9> }
/tmp/addcodecbug/node_modules/cids/node_modules/multicodec/src/index.js:76
    throw new Error('Codec `' + codecName + '` not found')
    ^

Error: Codec `my-codec` not found
    at Object.exports.getCodeVarint (/tmp/addcodecbug/node_modules/cids/node_modules/multicodec/src/index.js:76:11)
    at ClassIsWrapper.get buffer [as buffer] (/tmp/addcodecbug/node_modules/cids/src/index.js:131:22)
    at ClassIsWrapper.toBaseEncodedString (/tmp/addcodecbug/node_modules/cids/src/index.js:202:44)
    at Object.<anonymous> (/tmp/addcodecbug/index.js:17:17)
    at Module._compile (internal/modules/cjs/loader.js:702:30)
    at Object.Module._extensions..js (internal/modules/cjs/loader.js:713:10)
    at Module.load (internal/modules/cjs/loader.js:612:32)
    at tryModuleLoad (internal/modules/cjs/loader.js:551:12)
    at Function.Module._load (internal/modules/cjs/loader.js:543:3)
    at Function.Module.runMain (internal/modules/cjs/loader.js:744:10)
```



<a name="0.3.0"></a>
# [0.3.0](https://github.com/multiformats/js-multicodec/compare/v0.2.7...v0.3.0) (2018-12-18)


### Bug Fixes

* lint ([ac53866](https://github.com/multiformats/js-multicodec/commit/ac53866))


### Chores

* update the base table ([785519c](https://github.com/multiformats/js-multicodec/commit/785519c))


### BREAKING CHANGES

* 'murmur3' is now called 'murmur3-128'



<a name="0.2.7"></a>
## [0.2.7](https://github.com/multiformats/js-multicodec/compare/v0.2.6...v0.2.7) (2018-06-06)


### Bug Fixes

* throw error instead of returning `undefined` ([9859a71](https://github.com/multiformats/js-multicodec/commit/9859a71))


### Features

* add codec name to codec code helper ([46e0e02](https://github.com/multiformats/js-multicodec/commit/46e0e02))
* make adding a custom codec possible ([c6ee55b](https://github.com/multiformats/js-multicodec/commit/c6ee55b))



<a name="0.2.6"></a>
## [0.2.6](https://github.com/multiformats/js-multicodec/compare/v0.2.5...v0.2.6) (2018-01-20)



<a name="0.2.5"></a>
## [0.2.5](https://github.com/multiformats/js-multicodec/compare/v0.2.4...v0.2.5) (2017-10-11)



<a name="0.2.4"></a>
## [0.2.4](https://github.com/multiformats/js-multicodec/compare/v0.2.3...v0.2.4) (2017-10-06)


### Bug Fixes

* rename raw, mistake ([9cc4afc](https://github.com/multiformats/js-multicodec/commit/9cc4afc))



<a name="0.2.3"></a>
## [0.2.3](https://github.com/multiformats/js-multicodec/compare/v0.2.2...v0.2.3) (2017-10-06)



<a name="0.2.2"></a>
## [0.2.2](https://github.com/multiformats/js-multicodec/compare/v0.2.1...v0.2.2) (2017-10-06)



<a name="0.2.1"></a>
## [0.2.1](https://github.com/multiformats/js-multicodec/compare/v0.2.0...v0.2.1) (2017-10-06)



<a name="0.2.0"></a>
# [0.2.0](https://github.com/multiformats/js-multicodec/compare/v0.1.9...v0.2.0) (2017-10-06)


### Bug Fixes

* rename "raw" codec to "bin" to match multiformats table ([#19](https://github.com/multiformats/js-multicodec/issues/19)) ([4f97ded](https://github.com/multiformats/js-multicodec/commit/4f97ded))



<a name="0.1.9"></a>
## [0.1.9](https://github.com/multiformats/js-multicodec/compare/v0.1.8...v0.1.9) (2017-08-29)



<a name="0.1.8"></a>
## [0.1.8](https://github.com/multiformats/js-multicodec/compare/v0.1.7...v0.1.8) (2017-07-12)



<a name="0.1.7"></a>
## [0.1.7](https://github.com/multiformats/js-multicodec/compare/v0.1.6...v0.1.7) (2017-03-16)



<a name="0.1.6"></a>
## [0.1.6](https://github.com/multiformats/js-multicodec/compare/v0.1.5...v0.1.6) (2017-03-15)


### Features

* torrent codes ([#14](https://github.com/multiformats/js-multicodec/issues/14)) ([49c9c3d](https://github.com/multiformats/js-multicodec/commit/49c9c3d))



<a name="0.1.5"></a>
## [0.1.5](https://github.com/multiformats/js-multicodec/compare/v0.1.4...v0.1.5) (2017-02-09)



<a name="0.1.4"></a>
## [0.1.4](https://github.com/multiformats/js-multicodec/compare/v0.1.3...v0.1.4) (2017-02-09)



<a name="0.1.3"></a>
## [0.1.3](https://github.com/multiformats/js-multicodec/compare/v0.1.2...v0.1.3) (2016-12-30)



<a name="0.1.2"></a>
## [0.1.2](https://github.com/multiformats/js-multicodec/compare/v0.1.1...v0.1.2) (2016-12-06)


### Bug Fixes

* package.json ([f3a837d](https://github.com/multiformats/js-multicodec/commit/f3a837d))



<a name="0.1.1"></a>
## [0.1.1](https://github.com/multiformats/js-multicodec/compare/v0.1.0...v0.1.1) (2016-12-06)



<a name="0.1.0"></a>
# [0.1.0](https://github.com/multiformats/js-multicodec/compare/v0.0.1...v0.1.0) (2016-10-02)


### Bug Fixes

* getCodec, return codec ([0fff927](https://github.com/multiformats/js-multicodec/commit/0fff927))
* multicodec.getCodec func ([b60a3ac](https://github.com/multiformats/js-multicodec/commit/b60a3ac))


### Features

* add IPLD formats to table ([b4ac638](https://github.com/multiformats/js-multicodec/commit/b4ac638))



<a name="0.0.1"></a>
## [0.0.1](https://github.com/multiformats/js-multicodec/compare/bd02c0c...v0.0.1) (2016-09-26)


### Features

* codecs table + impl ([bd02c0c](https://github.com/multiformats/js-multicodec/commit/bd02c0c))



