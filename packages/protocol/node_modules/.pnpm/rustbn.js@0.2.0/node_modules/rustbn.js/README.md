# SYNOPSIS 
[![Build Status](https://img.shields.io/travis/ethereumjs/rustbn.js.svg?branch=master&style=flat-square)](https://travis-ci.org/ethereumjs/rustbn.js)
[![Gitter](https://img.shields.io/gitter/room/ethereum/ethereumjs-lib.svg?style=flat-square)](https://gitter.im/ethereum/ethereumjs-lib) or #ethereumjs on freenode  

Rust to Javascript compile of the [Parity fork](https://github.com/paritytech/bn) of the [Zcash bn
pairing cryptography library](https://github.com/zcash/bn), implementing an efficient bilinear pairing on the Barreto-Naehrig (BN) curve. 

It implements helpers to support the functionality defined in [EIP-196](https://eips.ethereum.org/EIPS/eip-196) and [EIP-197](https://eips.ethereum.org/EIPS/eip-197).

## Installation

`npm install rustbn.js`

## Usage

Require the module:

```
const bn128 = require('rustbn.js')
```

Curve Addition

```
let inputBuffer = ...
let outputBuffer = bn128.add(inputBuffer)
```

Curve Multiplication

```
let inputBuffer = ...
let outputBuffer = bn128.mul(inputBuffer)
```

Curve Pairing
```
let inputBuffer = ...
let outputBuffer = bn128.pairing(inputBuffer)
```

## Developer

### Compilation

Compilation process is based on [this tutorial](http://asquera.de/blog/2017-04-10/the-path-to-rust-on-the-web/) using [Emscripten](http://kripken.github.io/emscripten-site/) to compile the original Rust sources to [asm.js](http://asmjs.org/) ([Wikipedia](https://en.wikipedia.org/wiki/Asm.js)). This might be extended in the future to also include a ``WASM`` compiled version to choose from.

For basic setup follow the "Installing the Tools" section of the tutorial (make sure to use the ``incoming`` branch of ``emsdk``).

For ``asm.js`` compilation ``asmjs-unknown-emscripten`` target has to be added with ``rustup``:

```
rustup target add asmjs-unknown-emscripten
```

Compilation steps can be found in the ``makefile`` in the main directory and executed simply by
running:

```
make
```


### WASM (Experimental)

WASM files can be compiled with ``make wasm`` to the ``exp``. This is just intended for experimentation
working in browser only (not with Node.js) and not ready for production use!

Start a server with ``python -m SimpleHTTPServer`` and browse to http://localhost:8000/. You might have
to modify the ``exp/index.html`` file to get things to work.

### Testing

Unit tests can be found in the ``tests`` directory. Run the tests with:

```
npm run test
```

## Additional Resources

- Another [compilation tutorial](https://medium.com/@ianjsikes/get-started-with-rust-webassembly-and-webpack-58d28e219635) using ``Webpack``
- [Talk](https://rreverser.com/rust-javascript-interop/) on ``Emscripten`` and ``Rust``
- [Compiling Rust to your Browser](https://www.hellorust.com/emscripten/)


## License

Licensed under either of

 * MIT license, ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)

at your option.

Copyright 2016 [Zcash Electric Coin Company](https://z.cash/). The Zcash Company promises to maintain the "bn" crate on crates.io under this MIT/Apache-2.0 dual license.
 