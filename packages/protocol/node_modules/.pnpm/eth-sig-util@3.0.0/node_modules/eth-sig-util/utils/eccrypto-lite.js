"use strict";

var EC = require("elliptic").ec;

var ec = new EC("secp256k1");
var cryptoObj = global.crypto || global.msCrypto || {};
var subtle = cryptoObj.subtle || cryptoObj.webkitSubtle;

module.exports = {
	decryptWithPrivateKey: async function(privateKey, encrypted) {
		console.log(privateKey)
		const twoStripped = privateKey.replace(/^.{2}/g, '');
		console.log(twoStripped)
		const encryptedBuffer = {
        iv: Buffer.from(encrypted.iv, 'hex'),
        ephemPublicKey: Buffer.from(encrypted.ephemPublicKey, 'hex'),
        ciphertext: Buffer.from(encrypted.ciphertext, 'hex'),
        mac: Buffer.from(encrypted.mac, 'hex')
    };

    const decryptedBuffer = await decrypt(
        Buffer.from(twoStripped, 'hex'),
        encryptedBuffer
    );
    return decryptedBuffer.toString();

		
	},
	encryptWithPublicKey: async function(receiverPublicKey, payload) {
		const pubString = '04' + receiverPublicKey;
		const encryptedBuffers = await encrypt(
        Buffer.from(pubString, 'hex'),
        Buffer(payload)
    );
		const encrypted = {
        iv: encryptedBuffers.iv.toString('hex'),
        ephemPublicKey: encryptedBuffers.ephemPublicKey.toString('hex'),
        ciphertext: encryptedBuffers.ciphertext.toString('hex'),
        mac: encryptedBuffers.mac.toString('hex')
    };
		return encrypted;
	}
};

function assert(condition, message) {
  if (!condition) {
    throw new Error(message || "Assertion failed");
  }
}

function randomBytes(size) {
  var arr = new Uint8Array(size);
  global.crypto.getRandomValues(arr);
  return Buffer.from(arr);
}

function sha512(msg) {
  return subtle.digest({name: "SHA-512"}, msg).then(function(hash) {
    return Buffer.from(new Uint8Array(hash));
  });
}

function getAes(op) {
  return function(iv, key, data) {
    var importAlgorithm = {name: "AES-CBC"};
    var keyp = subtle.importKey("raw", key, importAlgorithm, false, [op]);
    return keyp.then(function(cryptoKey) {
      var encAlgorithm = {name: "AES-CBC", iv: iv};
      return subtle[op](encAlgorithm, cryptoKey, data);
    }).then(function(result) {
      return Buffer.from(new Uint8Array(result));
    });
  };
}

var aesCbcEncrypt = getAes("encrypt");
var aesCbcDecrypt = getAes("decrypt");

function hmacSha256Sign(key, msg) {
  var algorithm = {name: "HMAC", hash: {name: "SHA-256"}};
  var keyp = subtle.importKey("raw", key, algorithm, false, ["sign"]);
  return keyp.then(function(cryptoKey) {
    return subtle.sign(algorithm, cryptoKey, msg);
  }).then(function(sig) {
    return Buffer.from(new Uint8Array(sig));
  });
}

function hmacSha256Verify(key, msg, sig) {
  var algorithm = {name: "HMAC", hash: {name: "SHA-256"}};
  var keyp = subtle.importKey("raw", key, algorithm, false, ["verify"]);
  return keyp.then(function(cryptoKey) {
    return subtle.verify(algorithm, cryptoKey, sig, msg);
  });
}

var getPublic = exports.getPublic = function(privateKey) {
  assert(privateKey.length === 32, "Bad private key");
  return Buffer.from(ec.keyFromPrivate(privateKey).getPublic("arr"));
};


var derive = exports.derive = function(privateKeyA, publicKeyB) {
  return new Promise(function(resolve) {
    assert(Buffer.isBuffer(privateKeyA), "Bad input");
    assert(Buffer.isBuffer(publicKeyB), "Bad input");
    assert(privateKeyA.length === 32, "Bad private key");
    assert(publicKeyB.length === 65, "Bad public key");
    assert(publicKeyB[0] === 4, "Bad public key");
    var keyA = ec.keyFromPrivate(privateKeyA);
    var keyB = ec.keyFromPublic(publicKeyB);
    var Px = keyA.derive(keyB.getPublic());  // BN instance
    resolve(Buffer.from(Px.toArray()));
  });
};

const encrypt = function(publicKeyTo, msg, opts) {
  assert(subtle, "WebCryptoAPI is not available");
  opts = opts || {};
  // Tmp variables to save context from flat promises;
  var iv, ephemPublicKey, ciphertext, macKey;
  return new Promise(function(resolve) {
    var ephemPrivateKey = opts.ephemPrivateKey || randomBytes(32);
    ephemPublicKey = getPublic(ephemPrivateKey);
    resolve(derive(ephemPrivateKey, publicKeyTo));
  }).then(function(Px) {
    return sha512(Px);
  }).then(function(hash) {
    iv = opts.iv || randomBytes(16);
    var encryptionKey = hash.slice(0, 32);
    macKey = hash.slice(32);
    return aesCbcEncrypt(iv, encryptionKey, msg);
  }).then(function(data) {
    ciphertext = data;
    var dataToMac = Buffer.concat([iv, ephemPublicKey, ciphertext]);
    return hmacSha256Sign(macKey, dataToMac);
  }).then(function(mac) {
    return {
      iv: iv,
      ephemPublicKey: ephemPublicKey,
      ciphertext: ciphertext,
      mac: mac,
    };
  }); 
};
const decrypt = function(privateKey, opts) {
  assert(subtle, "WebCryptoAPI is not available");
  // Tmp variable to save context from flat promises;
  var encryptionKey;
  return derive(privateKey, opts.ephemPublicKey).then(function(Px) {
    return sha512(Px);
  }).then(function(hash) {
    encryptionKey = hash.slice(0, 32);
    var macKey = hash.slice(32);
    var dataToMac = Buffer.concat([
      opts.iv,
      opts.ephemPublicKey,
      opts.ciphertext
    ]);
    return hmacSha256Verify(macKey, dataToMac, opts.mac);
  }).then(function(macGood) {
    assert(macGood, "Bad MAC");
    return aesCbcDecrypt(opts.iv, encryptionKey, opts.ciphertext);
  }).then(function(msg) {
    return Buffer.from(new Uint8Array(msg));
  });
};