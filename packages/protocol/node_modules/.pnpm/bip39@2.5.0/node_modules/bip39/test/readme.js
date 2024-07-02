var bip39 = require('../')
var Buffer = require('safe-buffer').Buffer
var proxyquire = require('proxyquire')
var test = require('tape')

test('README example 1', function (t) {
  // defaults to BIP39 English word list
  var entropy = 'ffffffffffffffffffffffffffffffff'
  var mnemonic = bip39.entropyToMnemonic(entropy)

  t.plan(2)
  t.equal(mnemonic, 'zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong')

  // reversible
  t.equal(bip39.mnemonicToEntropy(mnemonic), entropy)
})

test('README example 2', function (t) {
  var stub = {
    randombytes: function (size) {
      return new Buffer('qwertyuiopasdfghjklzxcvbnm[];,./'.slice(0, size))
    }
  }
  var proxiedbip39 = proxyquire('../', stub)

  // mnemonic strength defaults to 128 bits
  var mnemonic = proxiedbip39.generateMnemonic()

  t.plan(2)
  t.equal(mnemonic, 'imitate robot frame trophy nuclear regret saddle around inflict case oil spice')
  t.equal(bip39.validateMnemonic(mnemonic), true)
})

test('README example 3', function (t) {
  var mnemonic = 'basket actual'
  var seed = bip39.mnemonicToSeed(mnemonic)
  var seedHex = bip39.mnemonicToSeedHex(mnemonic)

  t.plan(3)
  t.equal(seed.toString('hex'), seedHex)
  t.equal(seedHex, '5cf2d4a8b0355e90295bdfc565a022a409af063d5365bb57bf74d9528f494bfa4400f53d8349b80fdae44082d7f9541e1dba2b003bcfec9d0d53781ca676651f')
  t.equal(bip39.validateMnemonic(mnemonic), false)
})
