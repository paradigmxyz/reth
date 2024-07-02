"use strict";
var __assign = (this && this.__assign) || function () {
    __assign = Object.assign || function(t) {
        for (var s, i = 1, n = arguments.length; i < n; i++) {
            s = arguments[i];
            for (var p in s) if (Object.prototype.hasOwnProperty.call(s, p))
                t[p] = s[p];
        }
        return t;
    };
    return __assign.apply(this, arguments);
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.recoverTypedSignature_v4 = exports.recoverTypedSignature = exports.signTypedData_v4 = exports.signTypedData = exports.recoverTypedMessage = exports.signTypedMessage = exports.getEncryptionPublicKey = exports.decryptSafely = exports.decrypt = exports.encryptSafely = exports.encrypt = exports.recoverTypedSignatureLegacy = exports.signTypedDataLegacy = exports.typedSignatureHash = exports.extractPublicKey = exports.recoverPersonalSignature = exports.personalSign = exports.normalize = exports.concatSig = exports.TypedDataUtils = exports.TYPED_MESSAGE_SCHEMA = void 0;
var ethUtil = require("ethereumjs-util");
var ethAbi = require("ethereumjs-abi");
var nacl = require("tweetnacl");
var naclUtil = require("tweetnacl-util");
var TYPED_MESSAGE_SCHEMA = {
    type: 'object',
    properties: {
        types: {
            type: 'object',
            additionalProperties: {
                type: 'array',
                items: {
                    type: 'object',
                    properties: {
                        name: { type: 'string' },
                        type: { type: 'string' },
                    },
                    required: ['name', 'type'],
                },
            },
        },
        primaryType: { type: 'string' },
        domain: { type: 'object' },
        message: { type: 'object' },
    },
    required: ['types', 'primaryType', 'domain', 'message'],
};
exports.TYPED_MESSAGE_SCHEMA = TYPED_MESSAGE_SCHEMA;
/**
 * A collection of utility functions used for signing typed data
 */
var TypedDataUtils = {
    /**
     * Encodes an object by encoding and concatenating each of its members
     *
     * @param {string} primaryType - Root type
     * @param {Object} data - Object to encode
     * @param {Object} types - Type definitions
     * @returns {Buffer} - Encoded representation of an object
     */
    encodeData: function (primaryType, data, types, useV4) {
        var _this = this;
        if (useV4 === void 0) { useV4 = true; }
        var encodedTypes = ['bytes32'];
        var encodedValues = [this.hashType(primaryType, types)];
        if (useV4) {
            var encodeField_1 = function (name, type, value) {
                if (types[type] !== undefined) {
                    // eslint-disable-next-line no-eq-null
                    return ['bytes32', value == null ?
                            '0x0000000000000000000000000000000000000000000000000000000000000000' :
                            ethUtil.sha3(_this.encodeData(type, value, types, useV4))];
                }
                if (value === undefined) {
                    throw new Error("missing value for field " + name + " of type " + type);
                }
                if (type === 'bytes') {
                    return ['bytes32', ethUtil.sha3(value)];
                }
                if (type === 'string') {
                    // convert string to buffer - prevents ethUtil from interpreting strings like '0xabcd' as hex
                    if (typeof value === 'string') {
                        value = Buffer.from(value, 'utf8');
                    }
                    return ['bytes32', ethUtil.sha3(value)];
                }
                if (type.lastIndexOf(']') === type.length - 1) {
                    var parsedType_1 = type.slice(0, type.lastIndexOf('['));
                    var typeValuePairs = value.map(function (item) { return encodeField_1(name, parsedType_1, item); });
                    return ['bytes32', ethUtil.sha3(ethAbi.rawEncode(typeValuePairs.map(function (_a) {
                            var t = _a[0];
                            return t;
                        }), typeValuePairs.map(function (_a) {
                            var v = _a[1];
                            return v;
                        })))];
                }
                return [type, value];
            };
            for (var _i = 0, _a = types[primaryType]; _i < _a.length; _i++) {
                var field = _a[_i];
                var _b = encodeField_1(field.name, field.type, data[field.name]), type = _b[0], value = _b[1];
                encodedTypes.push(type);
                encodedValues.push(value);
            }
        }
        else {
            for (var _c = 0, _d = types[primaryType]; _c < _d.length; _c++) {
                var field = _d[_c];
                var value = data[field.name];
                if (value !== undefined) {
                    if (field.type === 'bytes') {
                        encodedTypes.push('bytes32');
                        value = ethUtil.sha3(value);
                        encodedValues.push(value);
                    }
                    else if (field.type === 'string') {
                        encodedTypes.push('bytes32');
                        // convert string to buffer - prevents ethUtil from interpreting strings like '0xabcd' as hex
                        if (typeof value === 'string') {
                            value = Buffer.from(value, 'utf8');
                        }
                        value = ethUtil.sha3(value);
                        encodedValues.push(value);
                    }
                    else if (types[field.type] !== undefined) {
                        encodedTypes.push('bytes32');
                        value = ethUtil.sha3(this.encodeData(field.type, value, types, useV4));
                        encodedValues.push(value);
                    }
                    else if (field.type.lastIndexOf(']') === field.type.length - 1) {
                        throw new Error('Arrays are unimplemented in encodeData; use V4 extension');
                    }
                    else {
                        encodedTypes.push(field.type);
                        encodedValues.push(value);
                    }
                }
            }
        }
        return ethAbi.rawEncode(encodedTypes, encodedValues);
    },
    /**
     * Encodes the type of an object by encoding a comma delimited list of its members
     *
     * @param {string} primaryType - Root type to encode
     * @param {Object} types - Type definitions
     * @returns {string} - Encoded representation of the type of an object
     */
    encodeType: function (primaryType, types) {
        var result = '';
        var deps = this.findTypeDependencies(primaryType, types).filter(function (dep) { return dep !== primaryType; });
        deps = [primaryType].concat(deps.sort());
        for (var _i = 0, deps_1 = deps; _i < deps_1.length; _i++) {
            var type = deps_1[_i];
            var children = types[type];
            if (!children) {
                throw new Error("No type definition specified: " + type);
            }
            result += type + "(" + types[type].map(function (_a) {
                var name = _a.name, t = _a.type;
                return t + " " + name;
            }).join(',') + ")";
        }
        return result;
    },
    /**
     * Finds all types within a type definition object
     *
     * @param {string} primaryType - Root type
     * @param {Object} types - Type definitions
     * @param {Array} results - current set of accumulated types
     * @returns {Array} - Set of all types found in the type definition
     */
    findTypeDependencies: function (primaryType, types, results) {
        if (results === void 0) { results = []; }
        primaryType = primaryType.match(/^\w*/u)[0];
        if (results.includes(primaryType) || types[primaryType] === undefined) {
            return results;
        }
        results.push(primaryType);
        for (var _i = 0, _a = types[primaryType]; _i < _a.length; _i++) {
            var field = _a[_i];
            for (var _b = 0, _c = this.findTypeDependencies(field.type, types, results); _b < _c.length; _b++) {
                var dep = _c[_b];
                !results.includes(dep) && results.push(dep);
            }
        }
        return results;
    },
    /**
     * Hashes an object
     *
     * @param {string} primaryType - Root type
     * @param {Object} data - Object to hash
     * @param {Object} types - Type definitions
     * @returns {Buffer} - Hash of an object
     */
    hashStruct: function (primaryType, data, types, useV4) {
        if (useV4 === void 0) { useV4 = true; }
        return ethUtil.sha3(this.encodeData(primaryType, data, types, useV4));
    },
    /**
     * Hashes the type of an object
     *
     * @param {string} primaryType - Root type to hash
     * @param {Object} types - Type definitions
     * @returns {Buffer} - Hash of an object
     */
    hashType: function (primaryType, types) {
        return ethUtil.sha3(this.encodeType(primaryType, types));
    },
    /**
     * Removes properties from a message object that are not defined per EIP-712
     *
     * @param {Object} data - typed message object
     * @returns {Object} - typed message object with only allowed fields
     */
    sanitizeData: function (data) {
        var sanitizedData = {};
        for (var key in TYPED_MESSAGE_SCHEMA.properties) {
            if (data[key]) {
                sanitizedData[key] = data[key];
            }
        }
        if ('types' in sanitizedData) {
            sanitizedData.types = __assign({ EIP712Domain: [] }, sanitizedData.types);
        }
        return sanitizedData;
    },
    /**
     * Signs a typed message as per EIP-712 and returns its sha3 hash
     *
     * @param {Object} typedData - Types message data to sign
     * @returns {Buffer} - sha3 hash of the resulting signed message
     */
    sign: function (typedData, useV4) {
        if (useV4 === void 0) { useV4 = true; }
        var sanitizedData = this.sanitizeData(typedData);
        var parts = [Buffer.from('1901', 'hex')];
        parts.push(this.hashStruct('EIP712Domain', sanitizedData.domain, sanitizedData.types, useV4));
        if (sanitizedData.primaryType !== 'EIP712Domain') {
            parts.push(this.hashStruct(sanitizedData.primaryType, sanitizedData.message, sanitizedData.types, useV4));
        }
        return ethUtil.sha3(Buffer.concat(parts));
    },
};
exports.TypedDataUtils = TypedDataUtils;
function concatSig(v, r, s) {
    var rSig = ethUtil.fromSigned(r);
    var sSig = ethUtil.fromSigned(s);
    var vSig = ethUtil.bufferToInt(v);
    var rStr = padWithZeroes(ethUtil.toUnsigned(rSig).toString('hex'), 64);
    var sStr = padWithZeroes(ethUtil.toUnsigned(sSig).toString('hex'), 64);
    var vStr = ethUtil.stripHexPrefix(ethUtil.intToHex(vSig));
    return ethUtil.addHexPrefix(rStr.concat(sStr, vStr)).toString('hex');
}
exports.concatSig = concatSig;
function normalize(input) {
    if (!input) {
        return undefined;
    }
    if (typeof input === 'number') {
        var buffer = ethUtil.toBuffer(input);
        input = ethUtil.bufferToHex(buffer);
    }
    if (typeof input !== 'string') {
        var msg = 'eth-sig-util.normalize() requires hex string or integer input.';
        msg += " received " + typeof input + ": " + input;
        throw new Error(msg);
    }
    return ethUtil.addHexPrefix(input.toLowerCase());
}
exports.normalize = normalize;
function personalSign(privateKey, msgParams) {
    var message = ethUtil.toBuffer(msgParams.data);
    var msgHash = ethUtil.hashPersonalMessage(message);
    var sig = ethUtil.ecsign(msgHash, privateKey);
    var serialized = ethUtil.bufferToHex(concatSig(sig.v, sig.r, sig.s));
    return serialized;
}
exports.personalSign = personalSign;
function recoverPersonalSignature(msgParams) {
    var publicKey = getPublicKeyFor(msgParams);
    var sender = ethUtil.publicToAddress(publicKey);
    var senderHex = ethUtil.bufferToHex(sender);
    return senderHex;
}
exports.recoverPersonalSignature = recoverPersonalSignature;
function extractPublicKey(msgParams) {
    var publicKey = getPublicKeyFor(msgParams);
    return "0x" + publicKey.toString('hex');
}
exports.extractPublicKey = extractPublicKey;
function externalTypedSignatureHash(typedData) {
    var hashBuffer = typedSignatureHash(typedData);
    return ethUtil.bufferToHex(hashBuffer);
}
exports.typedSignatureHash = externalTypedSignatureHash;
function signTypedDataLegacy(privateKey, msgParams) {
    var msgHash = typedSignatureHash(msgParams.data);
    var sig = ethUtil.ecsign(msgHash, privateKey);
    return ethUtil.bufferToHex(concatSig(sig.v, sig.r, sig.s));
}
exports.signTypedDataLegacy = signTypedDataLegacy;
function recoverTypedSignatureLegacy(msgParams) {
    var msgHash = typedSignatureHash(msgParams.data);
    var publicKey = recoverPublicKey(msgHash, msgParams.sig);
    var sender = ethUtil.publicToAddress(publicKey);
    return ethUtil.bufferToHex(sender);
}
exports.recoverTypedSignatureLegacy = recoverTypedSignatureLegacy;
function encrypt(receiverPublicKey, msgParams, version) {
    switch (version) {
        case 'x25519-xsalsa20-poly1305': {
            if (typeof msgParams.data !== 'string') {
                throw new Error('Cannot detect secret message, message params should be of the form {data: "secret message"} ');
            }
            // generate ephemeral keypair
            var ephemeralKeyPair = nacl.box.keyPair();
            // assemble encryption parameters - from string to UInt8
            var pubKeyUInt8Array = void 0;
            try {
                pubKeyUInt8Array = naclUtil.decodeBase64(receiverPublicKey);
            }
            catch (err) {
                throw new Error('Bad public key');
            }
            var msgParamsUInt8Array = naclUtil.decodeUTF8(msgParams.data);
            var nonce = nacl.randomBytes(nacl.box.nonceLength);
            // encrypt
            var encryptedMessage = nacl.box(msgParamsUInt8Array, nonce, pubKeyUInt8Array, ephemeralKeyPair.secretKey);
            // handle encrypted data
            var output = {
                version: 'x25519-xsalsa20-poly1305',
                nonce: naclUtil.encodeBase64(nonce),
                ephemPublicKey: naclUtil.encodeBase64(ephemeralKeyPair.publicKey),
                ciphertext: naclUtil.encodeBase64(encryptedMessage),
            };
            // return encrypted msg data
            return output;
        }
        default:
            throw new Error('Encryption type/version not supported');
    }
}
exports.encrypt = encrypt;
function encryptSafely(receiverPublicKey, msgParams, version) {
    var DEFAULT_PADDING_LENGTH = Math.pow(2, 11);
    var NACL_EXTRA_BYTES = 16;
    var data = msgParams.data;
    if (!data) {
        throw new Error('Cannot encrypt empty msg.data');
    }
    if (typeof data === 'object' && 'toJSON' in data) {
        // remove toJSON attack vector
        // TODO, check all possible children
        throw new Error('Cannot encrypt with toJSON property.  Please remove toJSON property');
    }
    // add padding
    var dataWithPadding = {
        data: data,
        padding: '',
    };
    // calculate padding
    var dataLength = Buffer.byteLength(JSON.stringify(dataWithPadding), 'utf-8');
    var modVal = dataLength % DEFAULT_PADDING_LENGTH;
    var padLength = 0;
    // Only pad if necessary
    if (modVal > 0) {
        padLength = DEFAULT_PADDING_LENGTH - modVal - NACL_EXTRA_BYTES; // nacl extra bytes
    }
    dataWithPadding.padding = '0'.repeat(padLength);
    var paddedMsgParams = { data: JSON.stringify(dataWithPadding) };
    return encrypt(receiverPublicKey, paddedMsgParams, version);
}
exports.encryptSafely = encryptSafely;
function decrypt(encryptedData, receiverPrivateKey) {
    switch (encryptedData.version) {
        case 'x25519-xsalsa20-poly1305': {
            // string to buffer to UInt8Array
            var recieverPrivateKeyUint8Array = nacl_decodeHex(receiverPrivateKey);
            var recieverEncryptionPrivateKey = nacl.box.keyPair.fromSecretKey(recieverPrivateKeyUint8Array).secretKey;
            // assemble decryption parameters
            var nonce = naclUtil.decodeBase64(encryptedData.nonce);
            var ciphertext = naclUtil.decodeBase64(encryptedData.ciphertext);
            var ephemPublicKey = naclUtil.decodeBase64(encryptedData.ephemPublicKey);
            // decrypt
            var decryptedMessage = nacl.box.open(ciphertext, nonce, ephemPublicKey, recieverEncryptionPrivateKey);
            // return decrypted msg data
            var output = void 0;
            try {
                output = naclUtil.encodeUTF8(decryptedMessage);
            }
            catch (err) {
                throw new Error('Decryption failed.');
            }
            if (output) {
                return output;
            }
            throw new Error('Decryption failed.');
        }
        default:
            throw new Error('Encryption type/version not supported.');
    }
}
exports.decrypt = decrypt;
function decryptSafely(encryptedData, receiverPrivateKey) {
    var dataWithPadding = JSON.parse(decrypt(encryptedData, receiverPrivateKey));
    return dataWithPadding.data;
}
exports.decryptSafely = decryptSafely;
function getEncryptionPublicKey(privateKey) {
    var privateKeyUint8Array = nacl_decodeHex(privateKey);
    var encryptionPublicKey = nacl.box.keyPair.fromSecretKey(privateKeyUint8Array).publicKey;
    return naclUtil.encodeBase64(encryptionPublicKey);
}
exports.getEncryptionPublicKey = getEncryptionPublicKey;
/**
 * A generic entry point for all typed data methods to be passed, includes a version parameter.
 */
function signTypedMessage(privateKey, msgParams, version) {
    if (version === void 0) { version = 'V4'; }
    switch (version) {
        case 'V1':
            return signTypedDataLegacy(privateKey, msgParams);
        case 'V3':
            return signTypedData(privateKey, msgParams);
        case 'V4':
        default:
            return signTypedData_v4(privateKey, msgParams);
    }
}
exports.signTypedMessage = signTypedMessage;
function recoverTypedMessage(msgParams, version) {
    if (version === void 0) { version = 'V4'; }
    switch (version) {
        case 'V1':
            return recoverTypedSignatureLegacy(msgParams);
        case 'V3':
            return recoverTypedSignature(msgParams);
        case 'V4':
        default:
            return recoverTypedSignature_v4(msgParams);
    }
}
exports.recoverTypedMessage = recoverTypedMessage;
function signTypedData(privateKey, msgParams) {
    var message = TypedDataUtils.sign(msgParams.data, false);
    var sig = ethUtil.ecsign(message, privateKey);
    return ethUtil.bufferToHex(concatSig(sig.v, sig.r, sig.s));
}
exports.signTypedData = signTypedData;
function signTypedData_v4(privateKey, msgParams) {
    var message = TypedDataUtils.sign(msgParams.data);
    var sig = ethUtil.ecsign(message, privateKey);
    return ethUtil.bufferToHex(concatSig(sig.v, sig.r, sig.s));
}
exports.signTypedData_v4 = signTypedData_v4;
function recoverTypedSignature(msgParams) {
    var message = TypedDataUtils.sign(msgParams.data, false);
    var publicKey = recoverPublicKey(message, msgParams.sig);
    var sender = ethUtil.publicToAddress(publicKey);
    return ethUtil.bufferToHex(sender);
}
exports.recoverTypedSignature = recoverTypedSignature;
function recoverTypedSignature_v4(msgParams) {
    var message = TypedDataUtils.sign(msgParams.data);
    var publicKey = recoverPublicKey(message, msgParams.sig);
    var sender = ethUtil.publicToAddress(publicKey);
    return ethUtil.bufferToHex(sender);
}
exports.recoverTypedSignature_v4 = recoverTypedSignature_v4;
/**
 * @param typedData - Array of data along with types, as per EIP712.
 * @returns Buffer
 */
function typedSignatureHash(typedData) {
    var error = new Error('Expect argument to be non-empty array');
    if (typeof typedData !== 'object' || !('length' in typedData) || !typedData.length) {
        throw error;
    }
    var data = typedData.map(function (e) {
        return e.type === 'bytes' ? ethUtil.toBuffer(e.value) : e.value;
    });
    var types = typedData.map(function (e) {
        return e.type;
    });
    var schema = typedData.map(function (e) {
        if (!e.name) {
            throw error;
        }
        return e.type + " " + e.name;
    });
    return ethAbi.soliditySHA3(['bytes32', 'bytes32'], [
        ethAbi.soliditySHA3(new Array(typedData.length).fill('string'), schema),
        ethAbi.soliditySHA3(types, data),
    ]);
}
function recoverPublicKey(hash, sig) {
    var signature = ethUtil.toBuffer(sig);
    var sigParams = ethUtil.fromRpcSig(signature);
    return ethUtil.ecrecover(hash, sigParams.v, sigParams.r, sigParams.s);
}
function getPublicKeyFor(msgParams) {
    var message = ethUtil.toBuffer(msgParams.data);
    var msgHash = ethUtil.hashPersonalMessage(message);
    return recoverPublicKey(msgHash, msgParams.sig);
}
function padWithZeroes(number, length) {
    var myString = "" + number;
    while (myString.length < length) {
        myString = "0" + myString;
    }
    return myString;
}
// converts hex strings to the Uint8Array format used by nacl
function nacl_decodeHex(msgHex) {
    var msgBase64 = Buffer.from(msgHex, 'hex').toString('base64');
    return naclUtil.decodeBase64(msgBase64);
}
