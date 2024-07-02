# Finalization mix

MurmurHash3 x86 finalization mix implemented in JavaScript.

## Installation

```sh
npm install --save fmix
```

## Usage

```js
const fmix = require('fmix')

console.log(fmix(0xdeadbeef))
//=> 233162409
```

## API

### fmix(input: number) => number

Returns the finalization mix value of the number `input`.

## See also

- [murmur-32](https://github.com/LinusU/murmur-32) - MurmurHash3 x86 32-bit in JavaScript
- [murmur-128](https://github.com/LinusU/murmur-128) - MurmurHash3 x86 128-bit in JavaScript
