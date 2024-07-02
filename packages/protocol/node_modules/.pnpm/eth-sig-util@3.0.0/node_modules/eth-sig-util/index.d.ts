/// <reference types="node" />
export declare type TypedData = string | EIP712TypedData | EIP712TypedData[];
interface EIP712TypedData {
    name: string;
    type: string;
    value: any;
}
export declare type Version = 'V1' | 'V2' | 'V3' | 'V4';
export interface EthEncryptedData {
    version: string;
    nonce: string;
    ephemPublicKey: string;
    ciphertext: string;
}
export declare type SignedMsgParams<D> = Required<MsgParams<D>>;
export interface MsgParams<D> {
    data: D;
    sig?: string;
}
interface MessageTypeProperty {
    name: string;
    type: string;
}
interface MessageTypes {
    EIP712Domain: MessageTypeProperty[];
    [additionalProperties: string]: MessageTypeProperty[];
}
export interface TypedMessage<T extends MessageTypes> {
    types: T;
    primaryType: keyof T;
    domain: {
        name?: string;
        version?: string;
        chainId?: number;
        verifyingContract?: string;
    };
    message: object;
}
declare const TYPED_MESSAGE_SCHEMA: {
    type: string;
    properties: {
        types: {
            type: string;
            additionalProperties: {
                type: string;
                items: {
                    type: string;
                    properties: {
                        name: {
                            type: string;
                        };
                        type: {
                            type: string;
                        };
                    };
                    required: string[];
                };
            };
        };
        primaryType: {
            type: string;
        };
        domain: {
            type: string;
        };
        message: {
            type: string;
        };
    };
    required: string[];
};
/**
 * A collection of utility functions used for signing typed data
 */
declare const TypedDataUtils: {
    /**
     * Encodes an object by encoding and concatenating each of its members
     *
     * @param {string} primaryType - Root type
     * @param {Object} data - Object to encode
     * @param {Object} types - Type definitions
     * @returns {Buffer} - Encoded representation of an object
     */
    encodeData(primaryType: string, data: object, types: object, useV4?: boolean): Buffer;
    /**
     * Encodes the type of an object by encoding a comma delimited list of its members
     *
     * @param {string} primaryType - Root type to encode
     * @param {Object} types - Type definitions
     * @returns {string} - Encoded representation of the type of an object
     */
    encodeType(primaryType: string, types: object): string;
    /**
     * Finds all types within a type definition object
     *
     * @param {string} primaryType - Root type
     * @param {Object} types - Type definitions
     * @param {Array} results - current set of accumulated types
     * @returns {Array} - Set of all types found in the type definition
     */
    findTypeDependencies(primaryType: string, types: object, results?: string[]): string[];
    /**
     * Hashes an object
     *
     * @param {string} primaryType - Root type
     * @param {Object} data - Object to hash
     * @param {Object} types - Type definitions
     * @returns {Buffer} - Hash of an object
     */
    hashStruct(primaryType: string, data: object, types: object, useV4?: boolean): Buffer;
    /**
     * Hashes the type of an object
     *
     * @param {string} primaryType - Root type to hash
     * @param {Object} types - Type definitions
     * @returns {Buffer} - Hash of an object
     */
    hashType(primaryType: string, types: object): Buffer;
    /**
     * Removes properties from a message object that are not defined per EIP-712
     *
     * @param {Object} data - typed message object
     * @returns {Object} - typed message object with only allowed fields
     */
    sanitizeData<T extends MessageTypes>(data: string | EIP712TypedData | EIP712TypedData[] | TypedMessage<T>): TypedMessage<T>;
    /**
     * Signs a typed message as per EIP-712 and returns its sha3 hash
     *
     * @param {Object} typedData - Types message data to sign
     * @returns {Buffer} - sha3 hash of the resulting signed message
     */
    sign<T_1 extends MessageTypes>(typedData: string | EIP712TypedData[] | Partial<EIP712TypedData> | Partial<TypedMessage<T_1>>, useV4?: boolean): Buffer;
};
declare function concatSig(v: Buffer, r: Buffer, s: Buffer): string;
declare function normalize(input: number | string): string;
declare function personalSign<T extends MessageTypes>(privateKey: Buffer, msgParams: MsgParams<TypedData | TypedMessage<T>>): string;
declare function recoverPersonalSignature<T extends MessageTypes>(msgParams: SignedMsgParams<TypedData | TypedMessage<T>>): string;
declare function extractPublicKey<T extends MessageTypes>(msgParams: SignedMsgParams<TypedData | TypedMessage<T>>): string;
declare function externalTypedSignatureHash(typedData: EIP712TypedData[]): string;
declare function signTypedDataLegacy<T extends MessageTypes>(privateKey: Buffer, msgParams: MsgParams<TypedData | TypedMessage<T>>): string;
declare function recoverTypedSignatureLegacy<T extends MessageTypes>(msgParams: SignedMsgParams<TypedData | TypedMessage<T>>): string;
declare function encrypt<T extends MessageTypes>(receiverPublicKey: string, msgParams: MsgParams<TypedData | TypedMessage<T>>, version: string): EthEncryptedData;
declare function encryptSafely<T extends MessageTypes>(receiverPublicKey: string, msgParams: MsgParams<TypedData | TypedMessage<T>>, version: string): EthEncryptedData;
declare function decrypt(encryptedData: EthEncryptedData, receiverPrivateKey: string): string;
declare function decryptSafely(encryptedData: EthEncryptedData, receiverPrivateKey: string): string;
declare function getEncryptionPublicKey(privateKey: string): string;
/**
 * A generic entry point for all typed data methods to be passed, includes a version parameter.
 */
declare function signTypedMessage<T extends MessageTypes>(privateKey: Buffer, msgParams: MsgParams<TypedData | TypedMessage<T>>, version?: Version): string;
declare function recoverTypedMessage<T extends MessageTypes>(msgParams: SignedMsgParams<TypedData | TypedMessage<T>>, version?: Version): string;
declare function signTypedData<T extends MessageTypes>(privateKey: Buffer, msgParams: MsgParams<TypedData | TypedMessage<T>>): string;
declare function signTypedData_v4<T extends MessageTypes>(privateKey: Buffer, msgParams: MsgParams<TypedData | TypedMessage<T>>): string;
declare function recoverTypedSignature<T extends MessageTypes>(msgParams: SignedMsgParams<TypedData | TypedMessage<T>>): string;
declare function recoverTypedSignature_v4<T extends MessageTypes>(msgParams: SignedMsgParams<TypedData | TypedMessage<T>>): string;
export { TYPED_MESSAGE_SCHEMA, TypedDataUtils, concatSig, normalize, personalSign, recoverPersonalSignature, extractPublicKey, externalTypedSignatureHash as typedSignatureHash, signTypedDataLegacy, recoverTypedSignatureLegacy, encrypt, encryptSafely, decrypt, decryptSafely, getEncryptionPublicKey, signTypedMessage, recoverTypedMessage, signTypedData, signTypedData_v4, recoverTypedSignature, recoverTypedSignature_v4, };
