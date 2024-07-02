/*
	ISC License

	Copyright (c) 2019, Pierre-Louis Despaigne

	Permission to use, copy, modify, and/or distribute this software for any
	purpose with or without fee is hereby granted, provided that the above
	copyright notice and this permission notice appear in all copies.

	THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
	WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
	MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
	ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
	WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
	ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
	OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
*/

const CID = require('cids');
const multiH = require('multihashes');

/**
 * Convert an hexadecimal string to a Buffer, the string can start with or without '0x'
 * @param {string} hex an hexadecimal value
 * @return {Buffer} the resulting Buffer
 */
const hexStringToBuffer = (hex) => {
	let prefix = hex.slice(0, 2);
	let value = hex.slice(2);
	let res = '';
	if (prefix === '0x') res = value;
	else res = hex;
	return multiH.fromHexString(res);
}

/**
* list of known encoding,
* encoding should be a function that takes a `string` input,
* and return a `Buffer` result
*/
const encodes = {
  /**
  * @param {string} value
  * @return {Buffer}
  */
  swarm: (value) => {
    const multihash = multiH.encode(hexStringToBuffer(value), 'keccak-256');
		return new CID(1, 'swarm-manifest', multihash).buffer;
  },
  /**
  * @param {string} value
  * @return {Buffer}
  */
  ipfs: (value) => {
    const multihash = multiH.fromB58String(value);
    return new CID(1, 'dag-pb', multihash).buffer;
  },
  /**
  * @param {string} value
  * @return {Buffer}
  */
  utf8: (value) => {
    return Buffer.from(value, 'utf8');
  },
};

/** 
* list of known decoding,
* decoding should be a function that takes a `Buffer` input,
* and return a `string` result
*/
const decodes = {
  /**
  * @param {Buffer} value 
  */
  hexMultiHash: (value) => {
    const cid = new CID(value);
    return multiH.decode(cid.multihash).digest.toString('hex');
  },
  /**
  * @param {Buffer} value 
  */
  b58MultiHash: (value) => {
    const cid = new CID(value);
    return multiH.toB58String(cid.multihash);
  },
  /**
  * @param {Buffer} value 
  */
  utf8: (value) => {
    return value.toString('utf8');
  },
};

/**
* list of known encoding/decoding for a given codec,
* `encode` should be chosen among the `encodes` functions
* `decode` should be chosen among the `decodes` functions
*/
const profiles = {
  'swarm-ns': {
    encode: encodes.swarm,
    decode: decodes.hexMultiHash,
  },
  'ipfs-ns': {
    encode: encodes.ipfs,
    decode: decodes.b58MultiHash,
  },
  'ipns-ns': {
    encode: encodes.ipfs,
    decode: decodes.b58MultiHash,
  },
  'default': {
    encode: encodes.utf8,
    decode: decodes.utf8,
  },
};

exports.hexStringToBuffer = hexStringToBuffer;
exports.profiles = profiles;