[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![JavaScript Style Guide](https://img.shields.io/badge/code_style-standard-brightgreen.svg)](https://standardjs.com)
[![Node.js CI](https://github.com/juanelas/bigint-crypto-utils/actions/workflows/build-and-test.yml/badge.svg)](https://github.com/juanelas/bigint-crypto-utils/actions/workflows/build-and-test.yml)
[![Coverage Status](https://coveralls.io/repos/github/juanelas/bigint-crypto-utils/badge.svg?branch=main)](https://coveralls.io/github/juanelas/bigint-crypto-utils?branch=main)

# bigint-crypto-utils

Arbitrary precision modular arithmetic, cryptographically secure random numbers and strong probable prime generation/testing.

It relies on the native JS implementation of ([BigInt](https://tc39.es/ecma262/#sec-bigint-objects)). It can be used by any [Web Browser or webview supporting BigInt](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/BigInt#Browser_compatibility) and with Node.js (>=10.4.0). The bundles can be imported directly by the browser or in Angular projects, React apps, Node.js, etc.

Secure random numbers are generated using the native crypto implementation of the browsers ([Web Cryptography API](https://w3c.github.io/webcrypto/)) or [Node.js Crypto](https://nodejs.org/dist/latest/docs/api/crypto.html). Strong probable prime generation and testing use Miller-Rabin primality tests and are automatically sped up using parallel workers both in browsers and Node.js.

> The operations supported on BigInts are not constant time. BigInt can be therefore **[unsuitable for use in cryptography](https://www.chosenplaintext.ca/articles/beginners-guide-constant-time-cryptography.html).** Many platforms provide native support for cryptography, such as [Web Cryptography API](https://w3c.github.io/webcrypto/) or [Node.js Crypto](https://nodejs.org/dist/latest/docs/api/crypto.html).

## Usage

`bigint-crypto-utils` can be imported to your project with `npm`:

```console
npm install bigint-crypto-utils
```

Then either require (Node.js CJS):

```javascript
const bigintCryptoUtils = require('bigint-crypto-utils')
```

or import (JavaScript ES module):

```javascript
import * as bigintCryptoUtils from 'bigint-crypto-utils'
```

The appropriate version for browser or node is automatically exported.

> `bigint-crypto-utils` uses [ES2020 BigInt](https://tc39.es/ecma262/#sec-bigint-objects), so take into account that:
>
> 1. If you experience issues using webpack/babel to create your production bundles, you may edit the supported browsers list and leave only [supported browsers and versions](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/BigInt#Browser_compatibility). The browsers list is usually located in your project's `package.json` or the `.browserslistrc` file.
> 2. In order to use `bigint-crypto-utils` with TypeScript you need to set `target`, and `lib` and `module` if in use, to `ES2020` in your project's `tsconfig.json`.

You can also download the [IIFE bundle](https://raw.githubusercontent.com/juanelas/bigint-crypto-utils/main/dist/bundle.iife.js), the [ESM bundle](https://raw.githubusercontent.com/juanelas/bigint-crypto-utils/main/dist/bundle.esm.min.js) or the [UMD bundle](https://raw.githubusercontent.com/juanelas/bigint-crypto-utils/main/dist/bundle.umd.js) and manually add it to your project, or, if you have already installed `bigint-crypto-utils` in your project, just get the bundles from `node_modules/bigint-crypto-utils/dist/bundles/`.

An example of usage could be (complete examples can be found in the [examples](https://github.com/juanelas/bigint-crypto-utils/tree/master/examples) directory):

```typescript
/* A BigInt with value 666 can be declared calling the bigint constructor as
BigInt('666') or with the shorter 666n.
Notice that you can also pass a number to the constructor, e.g. BigInt(666).
However, it is not recommended since values over 2**53 - 1 won't be safe but
no warning will be raised.
*/
const a = BigInt('5')
const b = BigInt('2')
const n = 19n

console.log(bigintCryptoUtils.modPow(a, b, n)) // prints 6

console.log(bigintCryptoUtils.modInv(2n, 5n)) // prints 3

console.log(bigintCryptoUtils.modInv(BigInt('3'), BigInt('5'))) // prints 2

console.log(bigintCryptoUtils.randBetween(2n ** 256n)) // prints a cryptographically secure random number between 1 and 2**256 (both included).

async function primeTesting (): void {
  // Let us print out a probable prime of 2048 bits
  console.log(await bigintCryptoUtils.prime(2048))

  // Testing if number is a probable prime (Miller-Rabin)
  const number = 27n
  const isPrime = await bigintCryptoUtils.isProbablyPrime(number)
  if (isPrime === true) {
    console.log(`${number} is prime`)
  } else {
    console.log(`${number} is composite`)
  }
}

primeTesting()

```

## API reference documentation

[Check the API](./docs/API.md)
