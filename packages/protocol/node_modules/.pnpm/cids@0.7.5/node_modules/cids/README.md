# js-cid

[![](https://img.shields.io/badge/made%20by-Protocol%20Labs-blue.svg?style=flat-square)](http://protocol.ai/)
[![](https://img.shields.io/badge/freenode-%23ipfs-blue.svg?style=flat-square)](http://webchat.freenode.net/?channels=%23ipfs)
[![Travis CI](https://flat.badgen.net/travis/multiformats/js-cid)](https://travis-ci.com/multiformats/js-cid)
[![Coverage Status](https://coveralls.io/repos/github/multiformats/js-cid/badge.svg?branch=master)](https://coveralls.io/github/multiformats/js-cid?branch=master)
[![Dependency Status](https://david-dm.org/multiformats/js-cid.svg?style=flat-square)](https://david-dm.org/multiformats/js-cid)
[![js-standard-style](https://img.shields.io/badge/code%20style-standard-brightgreen.svg?style=flat-square)](https://github.com/feross/standard)
[![Greenkeeper badge](https://badges.greenkeeper.io/multiformats/js-cid.svg)](https://greenkeeper.io/)

> [CID](https://github.com/multiformats/cid) implementation in JavaScript.

## Lead Maintainer

[Volker Mische](https://github.com/vmx)

## Table of Contents

- [Install](#install)
- [Usage](#usage)
- [API](#api)
- [Contribute](#contribute)
- [License](#license)

## Install

### In Node.js through npm

```bash
$ npm install --save cids
```

### Browser: Browserify, Webpack, other bundlers

The code published to npm that gets loaded on require is in fact an ES5 transpiled version with the right shims added. This means that you can require it and use with your favourite bundler without having to adjust asset management process.

```js
const CID = require('cids')
```

### In the Browser through `<script>` tag

Loading this module through a script tag will make the ```Cids``` obj available in the global namespace.

```
<script src="https://unpkg.com/cids/dist/index.min.js"></script>
<!-- OR -->
<script src="https://unpkg.com/cids/dist/index.js"></script>
```

#### Gotchas

You will need to use Node.js `Buffer` API compatible, if you are running inside the browser, you can access it by `multihash.Buffer` or you can install Feross's [Buffer](https://github.com/feross/buffer).

## Usage

You can create an instance from a CID string or CID Buffer

```js
const CID = require('cids')

const cid = new CID('bafybeig6xv5nwphfmvcnektpnojts33jqcuam7bmye2pb54adnrtccjlsu')

cid.version       // 1
cid.codec         // 'dag-pb'
cid.multibaseName // 'base32'
cid.toString()
// 'bafybeig6xv5nwphfmvcnektpnojts33jqcuam7bmye2pb54adnrtccjlsu'
```

or by specifying the [cid version](https://github.com/multiformats/cid#versions), [multicodec name](https://github.com/multiformats/multicodec/blob/master/table.csv) and [multihash](https://github.com/multiformats/multihash):

```js
const CID = require('cids')
const multihashing = require('multihashing-async')

const hash = await multihashing(Buffer.from('OMG!'), 'sha2-256')
const cid = new CID(1, 'dag-pb', hash)
console.log(cid.toString())
// bafybeig6xv5nwphfmvcnektpnojts33jqcuam7bmye2pb54adnrtccjlsu
```

The string form of v1 CIDs defaults to `base32` encoding (v0 CIDs are always `base58btc` encoded). When creating a new instance you can optionally specify the default multibase to use when calling `toBaseEncodedString()` or `toString()`


```js
const cid = new CID(1, 'raw', hash, 'base64')
console.log(cid.toString())
// mAXASIN69ets85WVE0ipva5M5b2mAqAZ8LME08PeAG2MxCSuV
```

If you construct an instance from a valid CID string, the base you provided will be preserved as the default.

```js
// e.g. a base64url encoded CID
const cid = new CID('uAXASIHJSUj5lkfuP5VPWf_VahvhARLRqPkF24QxY-lKaSqvV')
cid.toString()
// uAXASIHJSUj5lkfuP5VPWf_VahvhARLRqPkF24QxY-lKaSqvV
```


## API

### CID.isCID(cid)

Returns true if object is a valid CID instance, false if not valid.

It's important to use this method rather than `instanceof` checks in
order to handle CID objects from different versions of this module.

### CID.validateCID(cid)

Validates the different components (version, codec, multihash, multibaseName) of the CID
instance. Throws an `Error` if not valid.

### new CID(version, codec, multihash, [multibaseName])

`version` must be [either 0 or 1](https://github.com/multiformats/cid#versions).

`codec` must be a string of a valid [registered codec](https://github.com/multiformats/multicodec/blob/master/table.csv).

`multihash` must be a `Buffer` instance of a valid [multihash](https://github.com/multiformats/multihash).

`multibaseName` optional string. Must be a valid [multibase](https://github.com/multiformats/multibase/blob/master/multibase.csv) name. Default is `base58btc` for v0 CIDs or `base32` for v1 CIDs.

### new CID(baseEncodedString)

Additionally, you can instantiate an instance from a base encoded
string.

### new CID(Buffer)

Additionally, you can instantiate an instance from a buffer.

#### cid.codec

Property containing the codec string.

#### cid.version

Property containing the CID version integer.

#### cid.multihash

Property containing the multihash buffer.

#### cid.multibaseName

Property containing the default base to use when calling `.toString`

#### cid.buffer

Property containing the full CID encoded as a `Buffer`.

#### cid.prefix

Proprety containing a buffer of the CID version, codec, and the prefix
section of the multihash.

#### cid.toV0()

Returns the CID encoded in version 0. Only works for `dag-pb` codecs.

Throws if codec is not `dag-pb`.

#### cid.toV1()

Returns the CID encoded in version 1.

#### cid.toBaseEncodedString(base=this.multibaseName)

Returns a base encoded string of the CID. Defaults to the base encoding in `this.multibaseName`.

The value of `this.multibaseName` depends on how the instance was constructed:

1. If the CID was constructed from an object that already had a multibase (a string or an existing CID) then it retains that base.
2. If the CID was constructed from an object that _did not_ have a multibase (a buffer, or by passing only version + codec + multihash to the constructor), then `multibaseName` will be `base58btc` for a v0 CID or `base32` for a v1 CID.

#### cid.toString(base=this.multibaseName)

Shorthand for `cid.toBaseEncodedString` described above.

#### cid.equals(cid)

Compare cid instance. Returns true if CID's are identical, false if
otherwise.

## Contribute

[![](https://cdn.rawgit.com/jbenet/contribute-ipfs-gif/master/img/contribute.gif)](https://github.com/ipfs/community/blob/master/contributing.md)

Contributions welcome. Please check out [the issues](https://github.com/multiformats/js-cid/issues).

Check out our [contributing document](https://github.com/ipfs/community/blob/master/CONTRIBUTING_JS.md) for more information on how we work, and about contributing in general. Please be aware that all interactions related to multiformats are subject to the IPFS [Code of Conduct](https://github.com/ipfs/community/blob/master/code-of-conduct.md).

Small note: If editing the Readme, please conform to the [standard-readme](https://github.com/RichardLitt/standard-readme) specification.

## License

MIT
