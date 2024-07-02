# level-transcoder

**Encode data with built-in or custom encodings.** The successor to [`level-codec`][level-codec] that transcodes encodings from and to internal data formats supported by a database. This allows a database to store data in a format of its choice (Buffer, Uint8Array or String) with zero-effort support of known encodings. That includes other encoding interfaces in the ecosystem like [`abstract-encoding`][abstract-enc] and [`multiformats`][blockcodec].

[![level badge][level-badge]](https://github.com/Level/awesome)
[![npm](https://img.shields.io/npm/v/level-transcoder.svg)](https://www.npmjs.com/package/level-transcoder)
[![Node version](https://img.shields.io/node/v/level-transcoder.svg)](https://www.npmjs.com/package/level-transcoder)
[![Test](https://img.shields.io/github/workflow/status/Level/transcoder/Test?label=test)](https://github.com/Level/transcoder/actions/workflows/test.yml)
[![Coverage](https://img.shields.io/codecov/c/github/Level/transcoder?label=\&logo=codecov\&logoColor=fff)](https://codecov.io/gh/Level/transcoder)
[![Standard](https://img.shields.io/badge/standard-informational?logo=javascript\&logoColor=fff)](https://standardjs.com)
[![Common Changelog](https://common-changelog.org/badge.svg)](https://common-changelog.org)
[![Donate](https://img.shields.io/badge/donate-orange?logo=open-collective\&logoColor=fff)](https://opencollective.com/level)

## Usage

Create a transcoder, passing a desired format:

```js
const { Transcoder } = require('level-transcoder')

const transcoder1 = new Transcoder(['view'])
const transcoder2 = new Transcoder(['buffer'])
const transcoder3 = new Transcoder(['utf8'])
```

Then select an encoding and encode some data:

```js
// Uint8Array(3) [ 49, 50, 51 ]
console.log(transcoder1.encoding('json').encode(123))

// <Buffer 31 32 33>
console.log(transcoder2.encoding('json').encode(123))

// '123'
console.log(transcoder3.encoding('json').encode(123))
```

If the `Transcoder` constructor is given multiple formats then `Transcoder#encoding()` selects an encoding with the best fitting format. Consider a database like [`leveldown`][leveldown] which has the ability to return data as a Buffer or string. If an `encoding.decode(data)` function needs a string, we'll want to fetch that `data` from the database as a string. This avoids the cost of having to convert a Buffer to a string. So we'd use the following transcoder:

```js
const transcoder = new Transcoder(['buffer', 'utf8'])
```

Then, knowing for example that the return value of `JSON.stringify(data)` is a UTF-8 string which matches one of the given formats, the `'json'` encoding will return a string here:

```js
// '123'
console.log(transcoder.encoding('json').encode(123))
```

In contrast, data encoded as a `'view'` (for now that just means Uint8Array) would get transcoded into the `'buffer'` encoding. Copying of data is avoided where possible, like how the underlying ArrayBuffer of a view can be passed to `Buffer.from(..)` without a copy.

Lastly, encodings returned by `Transcoder#encoding()` have a `format` property to be used to forward information to an underlying store. For example: an input value of `{ x: 3 }` using the `'json'` encoding which has a `format` of `'utf8'`, can be forwarded as value `'{"x":3}'` with encoding `'utf8'`. Vice versa for output.

## Encodings

### Built-in Encodings

These encodings can be used out of the box and are to be selected by name.

In this table, the _input_ is what `encode()` accepts. The _format_ is what `encode()` returns as well as what `decode()` accepts. The _output_ is what `decode()` returns. The TypeScript typings of `level-transcoder` have generic type parameters with matching names: `TIn`, `TFormat` and `TOut`.

| Name                    | Input                      | Format     | Output          |
| :---------------------- | :------------------------- | :--------- | :-------------- |
| `'buffer'` <sup>1</sup> | Buffer, Uint8Array, String | `'buffer'` | Buffer          |
| `'view'`                | Uint8Array, Buffer, String | `'view'`   | Uint8Array      |
| `'utf8'`                | String, Buffer, Uint8Array | `'utf8'`   | String          |
| `'json'`                | Any JSON type              | `'utf8'`   | As input        |
| `'hex'`                 | String (hex), Buffer       | `'buffer'` | String (hex)    |
| `'base64'`              | String (base64), Buffer    | `'buffer'` | String (base64) |

<sup>1</sup> Aliased as `'binary'`. Use of this alias does not affect the ability to transcode.

### Transcoder Encodings

It's not necessary to use or reference the below encodings directly. They're listed here for implementation notes and to show how input and output is the same; it's the format that differs.

Custom encodings are transcoded in the same way and require no additional setup. For example: if a custom encoding has `{ name: 'example', format: 'utf8' }` then `level-transcoder` will create transcoder encodings on demand with names `'example+buffer'` and `'example+view'`.

| Name                         | Input                      | Format     | Output          |
| :--------------------------- | :------------------------- | :--------- | :-------------- |
| `'buffer+view'`              | Buffer, Uint8Array, String | `'view'`   | Buffer          |
| `'view+buffer'`              | Uint8Array, Buffer, String | `'buffer'` | Uint8Array      |
| `'utf8+view'`                | String, Buffer, Uint8Array | `'view'`   | String          |
| `'utf8+buffer'`              | String, Buffer, Uint8Array | `'buffer'` | String          |
| `'json+view'`                | Any JSON type              | `'view'`   | As input        |
| `'json+buffer'`              | Any JSON type              | `'buffer'` | As input        |
| `'hex+view'` <sup>1</sup>    | String (hex), Buffer       | `'view'`   | String (hex)    |
| `'base64+view'` <sup>1</sup> | String (base64), Buffer    | `'view'`   | String (base64) |

<sup>1</sup> Unlike other encodings that transcode to `'view'`, these depend on Buffer at the moment and thus don't work in browsers if a [shim](https://github.com/feross/buffer) is not included by JavaScript bundlers like Webpack and Browserify.

### Ecosystem Encodings

Various modules in the ecosystem, in and outside of Level, can be used with `level-transcoder` although they follow different interfaces. Common between the interfaces is that they have `encode()` and `decode()` methods. The terms "codec" and "encoding" are used interchangeably in the ecosystem. Passing these encodings through `Transcoder#encoding()` (which is done implicitly when used in an `abstract-level` database) results in normalized encoding objects as described further below.

| Module                                     | Format           | Interface                           | Named |
| :----------------------------------------- | :--------------- | :---------------------------------- | :---- |
| [`protocol-buffers`][protocol-buffers]     | `buffer`         | [`level-codec`][level-codec]        | ❌     |
| [`charwise`][charwise]                     | `utf8`           | [`level-codec`][level-codec]        | ✅     |
| [`bytewise`][bytewise]                     | `buffer`         | [`level-codec`][level-codec]        | ✅     |
| [`lexicographic-integer-encoding`][lexint] | `buffer`, `utf8` | [`level-codec`][level-codec]        | ✅     |
| [`abstract-encoding`][abstract-enc]        | `buffer`         | [`abstract-encoding`][abstract-enc] | ❌     |
| [`multiformats`][js-multiformats]          | `view`           | [`multiformats`][blockcodec]        | ✅     |

Those marked as not named are modules that export or generate encodings that don't have a `name` property (or `type` as an alias). We call these _anonymous encodings_. They can only be used as objects and not by name. Passing an anonymous encoding through `Transcoder#encoding()` does give it a `name` property for compatibility, but the value of `name` is not deterministic.

## API

### `Transcoder`

#### `transcoder = new Transcoder(formats)`

Create a new transcoder, providing the formats that are supported by a database (or other). The `formats` argument must be an array containing one or more of `'buffer'`, `'view'`, `'utf8'`. The returned `transcoder` instance is stateful, in that it contains a set of cached encoding objects.

#### `encoding = transcoder.encoding(encoding)`

Returns the given `encoding` argument as a normalized encoding object that follows the `level-transcoder` encoding interface. The `encoding` argument may be:

- A string to select a known encoding by its name
- An object that follows one of the following interfaces: [`level-transcoder`](#encoding-interface), [`level-codec`](https://github.com/Level/codec#encoding-format), [`abstract-encoding`][abstract-enc], [`multiformats`][blockcodec]
- A previously normalized encoding, such that `encoding(x)` equals `encoding(encoding(x))`.

Results are cached. If the `encoding` argument is an object and it has a name then subsequent calls can refer to that encoding by name.

Depending on the `formats` provided to the `Transcoder` constructor, this method may return a _transcoder encoding_ that translates the desired encoding from / to a supported format. Its `encode()` and `decode()` methods will have respectively the same input and output types as a non-transcoded encoding, but its `name` property will differ.

#### `encodings = transcoder.encodings()`

Get an array of encoding objects. This includes:

- Encodings for the `formats` that were passed to the `Transcoder` constructor
- Custom encodings that were passed to `transcoder.encoding()`
- Transcoder encodings for either.

### `Encoding`

#### `data = encoding.encode(data)`

Encode data.

#### `data = encoding.decode(data)`

Decode data.

#### `encoding.name`

Unique name. A string.

#### `encoding.commonName`

Common name, computed from `name`. If this encoding is a transcoder encoding, `name` will be for example `'json+view'` and `commonName` will be just `'json'`. Else `name` will equal `commonName`.

#### `encoding.format`

Name of the (lower-level) encoding used by the return value of `encode()`. One of `'buffer'`, `'view'`, `'utf8'`. If `name` equals `format` then the encoding can be assumed to be idempotent, such that `encode(x)` equals `encode(encode(x))`.

## Encoding Interface

Custom encodings must follow the following interface:

```ts
interface IEncoding<TIn, TFormat, TOut> {
  name: string
  format: 'buffer' | 'view' | 'utf8'
  encode: (data: TIn) => TFormat
  decode: (data: TFormat) => TOut
}
```

## Install

With [npm](https://npmjs.org) do:

```
npm install level-transcoder
```

## Contributing

[`Level/transcoder`](https://github.com/Level/transcoder) is an **OPEN Open Source Project**. This means that:

> Individuals making significant and valuable contributions are given commit-access to the project to contribute as they see fit. This project is more like an open wiki than a standard guarded open source project.

See the [Contribution Guide](https://github.com/Level/community/blob/master/CONTRIBUTING.md) for more details.

## Donate

Support us with a monthly donation on [Open Collective](https://opencollective.com/level) and help us continue our work.

## License

[MIT](LICENSE)

[level-badge]: https://leveljs.org/img/badge.svg

[level-codec]: https://github.com/Level/codec

[leveldown]: https://github.com/Level/leveldown

[protocol-buffers]: https://github.com/mafintosh/protocol-buffers

[charwise]: https://github.com/dominictarr/charwise

[bytewise]: https://github.com/deanlandolt/bytewise

[lexint]: https://github.com/vweevers/lexicographic-integer-encoding

[abstract-enc]: https://github.com/mafintosh/abstract-encoding

[js-multiformats]: https://github.com/multiformats/js-multiformats

[blockcodec]: https://github.com/multiformats/js-multiformats/blob/master/src/codecs/interface.ts
