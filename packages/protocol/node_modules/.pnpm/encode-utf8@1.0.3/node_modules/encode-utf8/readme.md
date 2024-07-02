# Encode UTF8

Turn a string into an ArrayBuffer by using the UTF8 encoding.

## Installation

```js
npm install --save encode-uf8
```

## Usage

```js
const encodeUtf8 = require('encode-utf8')

console.log(encodeUtf8('Hello, World!'))
//=> ArrayBuffer { byteLength: 13 }

console.log(encodeUtf8('🐵 🙈 🙉 🙊'))
//=> ArrayBuffer { byteLength: 19 }
```

## API

### `encodeUtf8(input: string): ArrayBuffer`

Returns an ArrayBuffer with the string represented as UTF8 encoded data.
