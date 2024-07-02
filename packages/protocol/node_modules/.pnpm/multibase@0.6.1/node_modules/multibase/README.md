js-multibase
============

[![](https://img.shields.io/badge/made%20by-Protocol%20Labs-blue.svg?style=flat-square)](https://protocol.ai)
[![](https://img.shields.io/badge/project-multiformats-blue.svg?style=flat-square)](https://github.com/multiformats/multiformats)
[![](https://img.shields.io/badge/freenode-%23ipfs-blue.svg?style=flat-square)](https://webchat.freenode.net/?channels=%23ipfs)
[![Dependency Status](https://david-dm.org/multiformats/js-multibase.svg?style=flat-square)](https://david-dm.org/multiformats/js-multibase)
[![codecov](https://img.shields.io/codecov/c/github/multiformats/js-multibase.svg?style=flat-square)](https://codecov.io/gh/multiformats/js-multibase)
[![Travis CI](https://flat.badgen.net/travis/multiformats/js-multibase)](https://travis-ci.com/multiformats/js-multibase)

> JavaScript implementation of the [multibase](https://github.com/multiformats/multibase) specification

## Lead Maintainer

[Oli Evans](https://github.com/olizilla)

## Table of Contents

- [Install](#install)
  - [In Node.js through npm](#in-nodejs-through-npm)
  - [Browser: Browserify, Webpack, other bundlers](#browser-browserify-webpack-other-bundlers)
  - [In the Browser through `<script>` tag](#in-the-browser-through-script-tag)
    - [Gotchas](#gotchas)
- [Usage](#usage)
  - [Example](#example)
- [API](#api)
  - [`multibase` - Prefixes an encoded buffer with its multibase code](#multibase---prefixes-an-encoded-buffer-with-its-multibase-code)
  - [`multibase.encode` - Encodes a buffer into one of the supported encodings, prefixing it with the multibase code](#multibaseencode---encodes-a-buffer-into-one-of-the-supported-encodings-prefixing-it-with-the-multibase-code)
  - [`multibase.decode` - Decodes a buffer or string](#multibasedecode---decodes-a-buffer-or-string)
  - [`multibase.isEncoded` - Checks if buffer or string is encoded](#multibaseisencoded---checks-if-buffer-or-string-is-encoded)
  - [`multibase.names` - Supported base encoding names](#multibasenames)
  - [`multibase.codes` - Supported base encoding codes](#multibasecodes)
  - [Supported Encodings, see `src/constants.js`](#supported-encodings-see-srcconstantsjs)
- [Architecture and Encoding/Decoding](#architecture-and-encodingdecoding)
- [Adding additional bases](#adding-additional-bases)
- [License](#license)

## Install

### In Node.js through npm

```bash
> npm install --save multibase
```

### Browser: Browserify, Webpack, other bundlers

The code published to npm that gets loaded on require is in fact an ES5 transpiled version with the right shims added. This means that you can require it and use with your favourite bundler without having to adjust asset management process.

```js
const multibase = require('multibase')
```


### In the Browser through `<script>` tag

Loading this module through a script tag will make the ```Multibase``` obj available in the global namespace.

```html
<script src="https://unpkg.com/multibase/dist/index.min.js"></script>
<!-- OR -->
<script src="https://unpkg.com/multibase/dist/index.js"></script>
```

#### Gotchas

You will need to use Node.js `Buffer` API compatible, if you are running inside the browser, you can access it by `multibase.Buffer` or you can load Feross's [Buffer](https://github.com/feross/buffer) module.

## Usage

### Example

```JavaScript
const multibase = require('multibase')

const encodedBuf = multibase.encode('base58btc', new Buffer('hey, how is it going'))

const decodedBuf = multibase.decode(encodedBuf)
console.log(decodedBuf.toString())
// hey, how is it going
```

## API
https://multiformats.github.io/js-multibase/

### `multibase` - Prefixes an encoded buffer with its multibase code

```
const multibased = multibase(<nameOrCode>, encodedBuf)
```

### `multibase.encode` - Encodes a buffer into one of the supported encodings, prefixing it with the multibase code

```JavaScript
const encodedBuf = multibase.encode(<nameOrCode>, <buf>)
```

### `multibase.decode` - Decodes a buffer or string

```JavaScript
const decodedBuf = multibase.decode(bufOrString)
```

### `multibase.isEncoded` - Checks if buffer or string is encoded

```JavaScript
const value = multibase.isEncoded(bufOrString)
// value is the name of the encoding if it is encoded, false otherwise
```

### `multibase.names`

A frozen `Array` of supported base encoding names.

### `multibase.codes`

A frozen `Array` of supported base encoding codes.

### Supported Encodings, see [`src/constants.js`](/src/constants.js)

## Architecture and Encoding/Decoding

Multibase package defines all the supported bases and the location of their implementation in the constants.js file. A base is a class with a name, a code, an implementation and an alphabet.
```js
class Base {
  constructor (name, code, implementation, alphabet) {
    //...
  }
  // ...
}
```
The ```implementation``` is an object where the encoding/decoding functions are implemented. It must take one argument, (the alphabet) following the [base-x module](https://github.com/cryptocoinjs/base-x) architecture.

The ```alphabet``` is the **ordered** set of defined symbols for a given base.

The idea behind this is that several bases may have implementations from different locations/modules so it's useful to have an object (and a summary) of all of them in one location (hence the constants.js).

All the supported bases are currently using the npm [base-x](https://github.com/cryptocoinjs/base-x) module as their implementation. It is using bitwise maipulation to go from one base to another, so this module does not support padding at the moment.

## Adding additional bases

If the base you are looking for is not supported yet in js-multibase and you know a good encoding/decoding algorithm, you can add support for this base easily by editing the constants.js file
(**you'll need to create an issue about that beforehand since a code and a canonical name have to be defined**):

```js
const baseX = require('base-x')
//const newPackage = require('your-package-name')

const constants = [
  ['base1', '1', '', '1'],
  ['base2', '0', baseX, '01'],
  ['base8', '7', baseX, '01234567'],
  // ... [ 'your-base-name', 'code-to-be-defined', newPackage, 'alphabet']
]
```
The required package defines the implementation of the encoding/decoding process. **It must comply by these rules** :
- `encode` and `decode` functions with to-be-encoded buffer as the only expected argument
- the require call use the `alphabet` given as an argument for the encoding/decoding process

*If no package is specified (such as for base1 in the above example, it means the base is not implemented yet)*

Adding a new base requires the tests to be updated. Test files to be updated are :
- constants.spec.js
```js
describe('constants', () => {
  it('constants indexed by name', () => {
    const names = constants.names
    expect(Object.keys(names).length).to.equal(constants-count) // currently 12
  })

  it('constants indexed by code', () => {
    const codes = constants.codes
    expect(Object.keys(codes).length).to.equal(constants-count)
  })
})
```

- multibase.spec.js
    - if the base is implemented
    ```js
    const supportedBases = [
      ['base2', 'yes mani !', '01111001011001010111001100100000011011010110000101101110011010010010000000100001'],
      ['base8', 'yes mani !', '7171312714403326055632220041'],
      ['base10', 'yes mani !', '9573277761329450583662625'],
      // ... ['your-base-name', 'what you want', 'expected output']
    ```
    - if the base is not implemented yet
    ```js
    const supportedBases = [
      // ... ['your-base-name']
    ```

## Contribute

Contributions welcome. Please check out [the issues](https://github.com/multiformats/js-multibase/issues).

Check out our [contributing document](https://github.com/multiformats/multiformats/blob/master/contributing.md) for more information on how we work, and about contributing in general. Please be aware that all interactions related to multiformats are subject to the IPFS [Code of Conduct](https://github.com/ipfs/community/blob/master/code-of-conduct.md).

Small note: If editing the README, please conform to the [standard-readme](https://github.com/RichardLitt/standard-readme) specification.

## License

[MIT](LICENSE) © 2016 Protocol Labs Inc.
