# MurmurHash3 128-bit

MurmurHash3 x86 128-bit implemented in JavaScript.

## Installation

```sh
npm install --save murmur-128
```

## Usage

```js
const murmur128 = require('murmur-128')

murmur128('linus')
//=> ArrayBuffer { 16 }

murmur128(new ArrayBuffer(10))
//=> ArrayBuffer { 16 }
```

## API

### murmur128(key: ArrayBuffer | string) => ArrayBuffer

Compute the 128-bit MurmurHash3 of the supplied `key`. If the `key` is given as
string it will be [encoded using the UTF8 encoding](https://github.com/LinusU/encode-utf8).

## See also

- [murmur-32](https://github.com/LinusU/murmur-32) - MurmurHash3 x86 32-bit implemented in JavaScript
