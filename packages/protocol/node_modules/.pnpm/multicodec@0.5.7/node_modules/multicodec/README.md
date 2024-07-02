# js-multicodec

[![](https://img.shields.io/badge/made%20by-Protocol%20Labs-blue.svg?style=flat-square)](https://protocol.ai)
[![](https://img.shields.io/badge/project-multiformats-blue.svg?style=flat-square)](https://github.com/multiformats/multiformats)
[![](https://img.shields.io/badge/freenode-%23ipfs-blue.svg?style=flat-square)](https://webchat.freenode.net/?channels=%23ipfs)
[![](https://img.shields.io/badge/readme%20style-standard-brightgreen.svg?style=flat-square)](https://github.com/RichardLitt/standard-readme)
[![Travis CI](https://flat.badgen.net/travis/multiformats/js-multicodec)](https://travis-ci.com/multiformats/js-multicodec)
[![Coverage Status](https://coveralls.io/repos/github/multiformats/js-multicodec/badge.svg?branch=master)](https://coveralls.io/github/multiformats/js-multiformats?branch=master)

> JavaScript implementation of the multicodec specification

## Lead Maintainer

[Henrique Dias](http://github.com/hacdias)

## Table of Contents

- [Install](#install)
- [Usage](#usage)
- [Updating the lookup table](#updating-the-lookup-table)
- [Contribute](#contribute)
- [License](#license)

## Install

```sh
> npm install multicodec
```

```JavaScript
const multicodec = require('multicodec')
```

## Usage

### Example

```JavaScript

const multicodec = require('multicodec')

const prefixedProtobuf = multicodec.addPrefix('protobuf', protobufBuffer)
// prefixedProtobuf 0x50...

// The multicodec codec values can be accessed directly:
console.log(multicodec.DAG_CBOR)
// 113

// To get the string representation of a codec, e.g. for error messages:
console.log(multicodec.print[113])
// dag-cbor
```

### API

https://multiformats.github.io/js-multicodec/

[multicodec default table](https://github.com/multiformats/multicodec/blob/master/table.csv)

## Updating the lookup table

Updating the lookup table is done with a script. The source of truth is the
[multicodec default table](https://github.com/multiformats/multicodec/blob/master/table.csv).
Update the table with running:

    node ./tools/update-table.js

## Contribute

Contributions welcome. Please check out [the issues](https://github.com/multiformats/js-multicodec/issues).

Check out our [contributing document](https://github.com/multiformats/multiformats/blob/master/contributing.md) for more information on how we work, and about contributing in general. Please be aware that all interactions related to multiformats are subject to the IPFS [Code of Conduct](https://github.com/ipfs/community/blob/master/code-of-conduct.md).

Small note: If editing the README, please conform to the [standard-readme](https://github.com/RichardLitt/standard-readme) specification.

## License

[MIT](LICENSE) © 2016 Protocol Labs Inc.
