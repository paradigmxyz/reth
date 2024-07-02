'use strict';

var _typeof = typeof Symbol === "function" && typeof Symbol.iterator === "symbol" ? function (obj) { return typeof obj; } : function (obj) { return obj && typeof Symbol === "function" && obj.constructor === Symbol && obj !== Symbol.prototype ? "symbol" : typeof obj; };

var Buffer = require('safe-buffer').Buffer;
var ethUtil = require('ethereumjs-util');
var crypto = require('crypto');
var randomBytes = require('randombytes');
var scryptsy = require('scryptsy');
var uuidv4 = require('uuid/v4');
var bs58check = require('bs58check');

function assert(val, msg) {
  if (!val) {
    throw new Error(msg || 'Assertion failed');
  }
}

function runCipherBuffer(cipher, data) {
  return Buffer.concat([cipher.update(data), cipher.final()]);
}

var Wallet = function Wallet(priv, pub) {
  if (priv && pub) {
    throw new Error('Cannot supply both a private and a public key to the constructor');
  }

  if (priv && !ethUtil.isValidPrivate(priv)) {
    throw new Error('Private key does not satisfy the curve requirements (ie. it is invalid)');
  }

  if (pub && !ethUtil.isValidPublic(pub)) {
    throw new Error('Invalid public key');
  }

  this._privKey = priv;
  this._pubKey = pub;
};

Object.defineProperty(Wallet.prototype, 'privKey', {
  get: function get() {
    assert(this._privKey, 'This is a public key only wallet');
    return this._privKey;
  }
});

Object.defineProperty(Wallet.prototype, 'pubKey', {
  get: function get() {
    if (!this._pubKey) {
      this._pubKey = ethUtil.privateToPublic(this.privKey);
    }
    return this._pubKey;
  }
});

Wallet.generate = function (icapDirect) {
  if (icapDirect) {
    var max = new ethUtil.BN('088f924eeceeda7fe92e1f5b0fffffffffffffff', 16);
    while (true) {
      var privKey = randomBytes(32);
      if (new ethUtil.BN(ethUtil.privateToAddress(privKey)).lte(max)) {
        return new Wallet(privKey);
      }
    }
  } else {
    return new Wallet(randomBytes(32));
  }
};

Wallet.generateVanityAddress = function (pattern) {
  if ((typeof pattern === 'undefined' ? 'undefined' : _typeof(pattern)) !== 'object') {
    pattern = new RegExp(pattern);
  }

  while (true) {
    var privKey = randomBytes(32);
    var address = ethUtil.privateToAddress(privKey);

    if (pattern.test(address.toString('hex'))) {
      return new Wallet(privKey);
    }
  }
};

Wallet.prototype.getPrivateKey = function () {
  return this.privKey;
};

Wallet.prototype.getPrivateKeyString = function () {
  return ethUtil.bufferToHex(this.getPrivateKey());
};

Wallet.prototype.getPublicKey = function () {
  return this.pubKey;
};

Wallet.prototype.getPublicKeyString = function () {
  return ethUtil.bufferToHex(this.getPublicKey());
};

Wallet.prototype.getAddress = function () {
  return ethUtil.publicToAddress(this.pubKey);
};

Wallet.prototype.getAddressString = function () {
  return ethUtil.bufferToHex(this.getAddress());
};

Wallet.prototype.getChecksumAddressString = function () {
  return ethUtil.toChecksumAddress(this.getAddressString());
};

// https://github.com/ethereum/wiki/wiki/Web3-Secret-Storage-Definition
Wallet.prototype.toV3 = function (password, opts) {
  assert(this._privKey, 'This is a public key only wallet');

  opts = opts || {};
  var salt = opts.salt || randomBytes(32);
  var iv = opts.iv || randomBytes(16);

  var derivedKey;
  var kdf = opts.kdf || 'scrypt';
  var kdfparams = {
    dklen: opts.dklen || 32,
    salt: salt.toString('hex')
  };

  if (kdf === 'pbkdf2') {
    kdfparams.c = opts.c || 262144;
    kdfparams.prf = 'hmac-sha256';
    derivedKey = crypto.pbkdf2Sync(Buffer.from(password), salt, kdfparams.c, kdfparams.dklen, 'sha256');
  } else if (kdf === 'scrypt') {
    // FIXME: support progress reporting callback
    kdfparams.n = opts.n || 262144;
    kdfparams.r = opts.r || 8;
    kdfparams.p = opts.p || 1;
    derivedKey = scryptsy(Buffer.from(password), salt, kdfparams.n, kdfparams.r, kdfparams.p, kdfparams.dklen);
  } else {
    throw new Error('Unsupported kdf');
  }

  var cipher = crypto.createCipheriv(opts.cipher || 'aes-128-ctr', derivedKey.slice(0, 16), iv);
  if (!cipher) {
    throw new Error('Unsupported cipher');
  }

  var ciphertext = runCipherBuffer(cipher, this.privKey);

  var mac = ethUtil.keccak256(Buffer.concat([derivedKey.slice(16, 32), Buffer.from(ciphertext, 'hex')]));

  return {
    version: 3,
    id: uuidv4({ random: opts.uuid || randomBytes(16) }),
    address: this.getAddress().toString('hex'),
    crypto: {
      ciphertext: ciphertext.toString('hex'),
      cipherparams: {
        iv: iv.toString('hex')
      },
      cipher: opts.cipher || 'aes-128-ctr',
      kdf: kdf,
      kdfparams: kdfparams,
      mac: mac.toString('hex')
    }
  };
};

Wallet.prototype.getV3Filename = function (timestamp) {
  /*
   * We want a timestamp like 2016-03-15T17-11-33.007598288Z. Date formatting
   * is a pain in Javascript, everbody knows that. We could use moment.js,
   * but decide to do it manually in order to save space.
   *
   * toJSON() returns a pretty close version, so let's use it. It is not UTC though,
   * but does it really matter?
   *
   * Alternative manual way with padding and Date fields: http://stackoverflow.com/a/7244288/4964819
   *
   */
  var ts = timestamp ? new Date(timestamp) : new Date();

  return ['UTC--', ts.toJSON().replace(/:/g, '-'), '--', this.getAddress().toString('hex')].join('');
};

Wallet.prototype.toV3String = function (password, opts) {
  return JSON.stringify(this.toV3(password, opts));
};

Wallet.fromPublicKey = function (pub, nonStrict) {
  if (nonStrict) {
    pub = ethUtil.importPublic(pub);
  }
  return new Wallet(null, pub);
};

Wallet.fromExtendedPublicKey = function (pub) {
  assert(pub.slice(0, 4) === 'xpub', 'Not an extended public key');
  pub = bs58check.decode(pub).slice(45);
  // Convert to an Ethereum public key
  return Wallet.fromPublicKey(pub, true);
};

Wallet.fromPrivateKey = function (priv) {
  return new Wallet(priv);
};

Wallet.fromExtendedPrivateKey = function (priv) {
  assert(priv.slice(0, 4) === 'xprv', 'Not an extended private key');
  var tmp = bs58check.decode(priv);
  assert(tmp[45] === 0, 'Invalid extended private key');
  return Wallet.fromPrivateKey(tmp.slice(46));
};

// https://github.com/ethereum/go-ethereum/wiki/Passphrase-protected-key-store-spec
Wallet.fromV1 = function (input, password) {
  assert(typeof password === 'string');
  var json = (typeof input === 'undefined' ? 'undefined' : _typeof(input)) === 'object' ? input : JSON.parse(input);

  if (json.Version !== '1') {
    throw new Error('Not a V1 wallet');
  }

  if (json.Crypto.KeyHeader.Kdf !== 'scrypt') {
    throw new Error('Unsupported key derivation scheme');
  }

  var kdfparams = json.Crypto.KeyHeader.KdfParams;
  var derivedKey = scryptsy(Buffer.from(password), Buffer.from(json.Crypto.Salt, 'hex'), kdfparams.N, kdfparams.R, kdfparams.P, kdfparams.DkLen);

  var ciphertext = Buffer.from(json.Crypto.CipherText, 'hex');

  var mac = ethUtil.keccak256(Buffer.concat([derivedKey.slice(16, 32), ciphertext]));

  if (mac.toString('hex') !== json.Crypto.MAC) {
    throw new Error('Key derivation failed - possibly wrong passphrase');
  }

  var decipher = crypto.createDecipheriv('aes-128-cbc', ethUtil.keccak256(derivedKey.slice(0, 16)).slice(0, 16), Buffer.from(json.Crypto.IV, 'hex'));
  var seed = runCipherBuffer(decipher, ciphertext);

  return new Wallet(seed);
};

Wallet.fromV3 = function (input, password, nonStrict) {
  assert(typeof password === 'string');
  var json = (typeof input === 'undefined' ? 'undefined' : _typeof(input)) === 'object' ? input : JSON.parse(nonStrict ? input.toLowerCase() : input);

  if (json.version !== 3) {
    throw new Error('Not a V3 wallet');
  }

  var derivedKey;
  var kdfparams;
  if (json.crypto.kdf === 'scrypt') {
    kdfparams = json.crypto.kdfparams;

    // FIXME: support progress reporting callback
    derivedKey = scryptsy(Buffer.from(password), Buffer.from(kdfparams.salt, 'hex'), kdfparams.n, kdfparams.r, kdfparams.p, kdfparams.dklen);
  } else if (json.crypto.kdf === 'pbkdf2') {
    kdfparams = json.crypto.kdfparams;

    if (kdfparams.prf !== 'hmac-sha256') {
      throw new Error('Unsupported parameters to PBKDF2');
    }

    derivedKey = crypto.pbkdf2Sync(Buffer.from(password), Buffer.from(kdfparams.salt, 'hex'), kdfparams.c, kdfparams.dklen, 'sha256');
  } else {
    throw new Error('Unsupported key derivation scheme');
  }

  var ciphertext = Buffer.from(json.crypto.ciphertext, 'hex');

  var mac = ethUtil.keccak256(Buffer.concat([derivedKey.slice(16, 32), ciphertext]));
  if (mac.toString('hex') !== json.crypto.mac) {
    throw new Error('Key derivation failed - possibly wrong passphrase');
  }

  var decipher = crypto.createDecipheriv(json.crypto.cipher, derivedKey.slice(0, 16), Buffer.from(json.crypto.cipherparams.iv, 'hex'));
  var seed = runCipherBuffer(decipher, ciphertext);

  return new Wallet(seed);
};

/*
 * Based on https://github.com/ethereum/pyethsaletool/blob/master/pyethsaletool.py
 * JSON fields: encseed, ethaddr, btcaddr, email
 */
Wallet.fromEthSale = function (input, password) {
  assert(typeof password === 'string');
  var json = (typeof input === 'undefined' ? 'undefined' : _typeof(input)) === 'object' ? input : JSON.parse(input);

  var encseed = Buffer.from(json.encseed, 'hex');

  // key derivation
  var derivedKey = crypto.pbkdf2Sync(password, password, 2000, 32, 'sha256').slice(0, 16);

  // seed decoding (IV is first 16 bytes)
  // NOTE: crypto (derived from openssl) when used with aes-*-cbc will handle PKCS#7 padding internally
  //       see also http://stackoverflow.com/a/31614770/4964819
  var decipher = crypto.createDecipheriv('aes-128-cbc', derivedKey, encseed.slice(0, 16));
  var seed = runCipherBuffer(decipher, encseed.slice(16));

  var wallet = new Wallet(ethUtil.keccak256(seed));
  if (wallet.getAddress().toString('hex') !== json.ethaddr) {
    throw new Error('Decoded key mismatch - possibly wrong passphrase');
  }
  return wallet;
};

module.exports = Wallet;