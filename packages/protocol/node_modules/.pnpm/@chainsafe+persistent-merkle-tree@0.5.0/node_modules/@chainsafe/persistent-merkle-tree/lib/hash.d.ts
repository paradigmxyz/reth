import { HashObject } from "@chainsafe/as-sha256";
/**
 * Hash two 32 byte arrays
 */
export declare function hash(a: Uint8Array, b: Uint8Array): Uint8Array;
/**
 * Hash 2 objects, each store 8 numbers (equivalent to Uint8Array(32))
 */
export declare function hashTwoObjects(a: HashObject, b: HashObject): HashObject;
export declare function hashObjectToUint8Array(obj: HashObject): Uint8Array;
export declare function uint8ArrayToHashObject(byteArr: Uint8Array): HashObject;
export declare function isHashObject(hash: HashObject | Uint8Array): hash is HashObject;
