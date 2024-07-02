import { HashObject, byteArrayToHashObject, hashObjectToByteArray } from "./hashObject";
import SHA256 from "./sha256";
export { HashObject, byteArrayToHashObject, hashObjectToByteArray, SHA256 };
export declare function digest(data: Uint8Array): Uint8Array;
export declare function digest64(data: Uint8Array): Uint8Array;
export declare function digest2Bytes32(bytes1: Uint8Array, bytes2: Uint8Array): Uint8Array;
/**
 * Digest 2 objects, each has 8 properties from h0 to h7.
 * The performance is a little bit better than digest64 due to the use of Uint32Array
 * and the memory is a little bit better than digest64 due to no temporary Uint8Array.
 * @returns
 */
export declare function digest64HashObjects(obj1: HashObject, obj2: HashObject): HashObject;
//# sourceMappingURL=index.d.ts.map