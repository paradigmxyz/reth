/**
 * Compute the 128-bit MurmurHash3 of the supplied `key`. If the `key` is given as string it will be [encoded using the UTF8 encoding](https://github.com/LinusU/encode-utf8).
 */
declare function murmur128 (key: ArrayBuffer | string): ArrayBuffer

export = murmur128
