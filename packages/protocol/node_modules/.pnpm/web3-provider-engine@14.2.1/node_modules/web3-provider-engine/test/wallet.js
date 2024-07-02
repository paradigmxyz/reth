const test = require('tape')
const Transaction = require('ethereumjs-tx')
const ethUtil = require('ethereumjs-util')
const ProviderEngine = require('../index.js')
const FixtureProvider = require('../subproviders/fixture.js')
const NonceTracker = require('../subproviders/nonce-tracker.js')
const HookedWalletProvider = require('../subproviders/hooked-wallet.js')
const HookedWalletTxProvider = require('../subproviders/hooked-wallet-ethtx.js')
const TestBlockProvider = require('./util/block.js')
const createPayload = require('../util/create-payload.js')
const injectMetrics = require('./util/inject-metrics')


test('tx sig', function(t){
  t.plan(12)

  var privateKey = Buffer.from('cccd8f4d88de61f92f3747e4a9604a0395e6ad5138add4bec4a2ddf231ee24f9', 'hex')
  var address = Buffer.from('1234362ef32bcd26d3dd18ca749378213625ba0b', 'hex')
  var addressHex = '0x'+address.toString('hex')

  // sign all tx's
  var providerA = injectMetrics(new HookedWalletProvider({
    getAccounts: function(cb){
      cb(null, [addressHex])
    },
    signTransaction: function(txParams, cb){
      var tx = new Transaction(txParams)
      tx.sign(privateKey)
      var rawTx = '0x'+tx.serialize().toString('hex')
      cb(null, rawTx)
    },
  }))

  // handle nonce requests
  var providerB = injectMetrics(new NonceTracker())
  // handle all bottom requests
  var providerC = injectMetrics(new FixtureProvider({
    eth_gasPrice: '0x1234',
    eth_getTransactionCount: '0x00',
    eth_sendRawTransaction: function(payload, next, done){
      var rawTx = ethUtil.toBuffer(payload.params[0])
      var tx = new Transaction(rawTx)
      var hash = '0x'+tx.hash().toString('hex')
      done(null, hash)
    },
  }))
  // handle block requests
  var providerD = injectMetrics(new TestBlockProvider())

  var engine = new ProviderEngine()
  engine.addProvider(providerA)
  engine.addProvider(providerB)
  engine.addProvider(providerC)
  engine.addProvider(providerD)

  var txPayload = {
    method: 'eth_sendTransaction',
    params: [{
      from: addressHex,
      to: addressHex,
      value: '0x01',
      gas: '0x1234567890',
    }]
  }

  engine.start()
  engine.sendAsync(createPayload(txPayload), function(err, response){
    t.ifError(err, 'did not error')
    t.ok(response, 'has response')

    // intial tx request
    t.equal(providerA.getWitnessed('eth_sendTransaction').length, 1, 'providerA did see "signTransaction"')
    t.equal(providerA.getHandled('eth_sendTransaction').length, 1, 'providerA did handle "signTransaction"')

    // tx nonce
    t.equal(providerB.getWitnessed('eth_getTransactionCount').length, 1, 'providerB did see "eth_getTransactionCount"')
    t.equal(providerB.getHandled('eth_getTransactionCount').length, 0, 'providerB did NOT handle "eth_getTransactionCount"')
    t.equal(providerC.getWitnessed('eth_getTransactionCount').length, 1, 'providerC did see "eth_getTransactionCount"')
    t.equal(providerC.getHandled('eth_getTransactionCount').length, 1, 'providerC did handle "eth_getTransactionCount"')

    // gas price
    t.equal(providerC.getWitnessed('eth_gasPrice').length, 1, 'providerB did see "eth_gasPrice"')
    t.equal(providerC.getHandled('eth_gasPrice').length, 1, 'providerB did handle "eth_gasPrice"')

    // send raw tx
    t.equal(providerC.getWitnessed('eth_sendRawTransaction').length, 1, 'providerC did see "eth_sendRawTransaction"')
    t.equal(providerC.getHandled('eth_sendRawTransaction').length, 1, 'providerC did handle "eth_sendRawTransaction"')

    engine.stop()
    t.end()
  })

})

test('no such account', function(t){
  t.plan(1)

  var addressHex = '0x1234362ef32bcd26d3dd18ca749378213625ba0b'
  var otherAddressHex = '0x4321362ef32bcd26d3dd18ca749378213625ba0c'

  // sign all tx's
  var providerA = injectMetrics(new HookedWalletProvider({
    getAccounts: function(cb){
      cb(null, [addressHex])
    },
  }))

  // handle nonce requests
  var providerB = injectMetrics(new NonceTracker())
  // handle all bottom requests
  var providerC = injectMetrics(new FixtureProvider({
    eth_gasPrice: '0x1234',
    eth_getTransactionCount: '0x00',
    eth_sendRawTransaction: function(payload, next, done){
      var rawTx = ethUtil.toBuffer(payload.params[0])
      var tx = new Transaction(rawTx)
      var hash = '0x'+tx.hash().toString('hex')
      done(null, hash)
    },
  }))
  // handle block requests
  var providerD = injectMetrics(new TestBlockProvider())

  var engine = new ProviderEngine()
  engine.addProvider(providerA)
  engine.addProvider(providerB)
  engine.addProvider(providerC)
  engine.addProvider(providerD)

  var txPayload = {
    method: 'eth_sendTransaction',
    params: [{
      from: otherAddressHex,
      to: addressHex,
      value: '0x01',
      gas: '0x1234567890',
    }]
  }

  engine.start()
  engine.sendAsync(createPayload(txPayload), function(err, response){
    t.ok(err, 'did error')

    engine.stop()
    t.end()
  })

})


test('sign message', function(t){
  t.plan(3)

  var privateKey = Buffer.from('cccd8f4d88de61f92f3747e4a9604a0395e6ad5138add4bec4a2ddf231ee24f9', 'hex')
  var addressHex = '0x1234362ef32bcd26d3dd18ca749378213625ba0b'

  var message = 'haay wuurl'
  var signature = '0x68dc980608bceb5f99f691e62c32caccaee05317309015e9454eba1a14c3cd4505d1dd098b8339801239c9bcaac3c4df95569dcf307108b92f68711379be14d81c'

  // sign all messages
  var providerA = injectMetrics(new HookedWalletTxProvider({
    getAccounts: function(cb){
      cb(null, [addressHex])
    },
    getPrivateKey: function(address, cb){
      cb(null, privateKey)
    },
  }))

  // handle block requests
  var providerB = injectMetrics(new TestBlockProvider())

  var engine = new ProviderEngine()
  engine.addProvider(providerA)
  engine.addProvider(providerB)

  var payload = {
    method: 'eth_sign',
    params: [
      addressHex,
      message,
    ],
  }

  engine.start()
  engine.sendAsync(createPayload(payload), function(err, response){
    t.ifError(err, 'did not error')
    t.ok(response, 'has response')

    t.equal(response.result, signature, 'signed response is correct')

    engine.stop()
    t.end()
  })

})

// personal_sign was declared without an explicit set of test data
// so I made a script out of geth's internals to create this test data
// https://gist.github.com/kumavis/461d2c0e9a04ea0818e423bb77e3d260

signatureTest({
  testLabel: 'kumavis fml manual test I',
  method: 'personal_sign',
  // "hello world"
  message: '0x68656c6c6f20776f726c64',
  signature: '0xce909e8ea6851bc36c007a0072d0524b07a3ff8d4e623aca4c71ca8e57250c4d0a3fc38fa8fbaaa81ead4b9f6bd03356b6f8bf18bccad167d78891636e1d69561b',
  addressHex: '0xbe93f9bacbcffc8ee6663f2647917ed7a20a57bb',
  privateKey: Buffer.from('6969696969696969696969696969696969696969696969696969696969696969', 'hex'),
})

signatureTest({
  testLabel: 'kumavis fml manual test II',
  method: 'personal_sign',
  // some random binary message from parity's test
  message: '0x0cc175b9c0f1b6a831c399e26977266192eb5ffee6ae2fec3ad71c777531578f',
  signature: '0x9ff8350cc7354b80740a3580d0e0fd4f1f02062040bc06b893d70906f8728bb5163837fd376bf77ce03b55e9bd092b32af60e86abce48f7b8d3539988ee5a9be1c',
  addressHex: '0xbe93f9bacbcffc8ee6663f2647917ed7a20a57bb',
  privateKey: Buffer.from('6969696969696969696969696969696969696969696969696969696969696969', 'hex'),
})

signatureTest({
  testLabel: 'kumavis fml manual test III',
  method: 'personal_sign',
  // random binary message data and pk from parity's test
  // https://github.com/ethcore/parity/blob/5369a129ae276d38f3490abb18c5093b338246e0/rpc/src/v1/tests/mocked/eth.rs#L301-L317
  // note: their signature result is incorrect (last byte moved to front) due to a parity bug
  message: '0x0cc175b9c0f1b6a831c399e26977266192eb5ffee6ae2fec3ad71c777531578f',
  signature: '0xa2870db1d0c26ef93c7b72d2a0830fa6b841e0593f7186bc6c7cc317af8cf3a42fda03bd589a49949aa05db83300cdb553116274518dbe9d90c65d0213f4af491b',
  addressHex: '0xe0da1edcea030875cd0f199d96eb70f6ab78faf2',
  privateKey: Buffer.from('4545454545454545454545454545454545454545454545454545454545454545', 'hex'),
})

recoverTest({
  testLabel: 'geth kumavis manual I recover',
  method: 'personal_ecRecover',
  // "hello world"
  message: '0x68656c6c6f20776f726c64',
  signature: '0xce909e8ea6851bc36c007a0072d0524b07a3ff8d4e623aca4c71ca8e57250c4d0a3fc38fa8fbaaa81ead4b9f6bd03356b6f8bf18bccad167d78891636e1d69561b',
  addressHex: '0xbe93f9bacbcffc8ee6663f2647917ed7a20a57bb',
})

recoverTest({
  testLabel: 'geth kumavis manual II recover',
  method: 'personal_ecRecover',
  // message from parity's test - note result is different than what they are testing against
  // https://github.com/ethcore/parity/blob/5369a129ae276d38f3490abb18c5093b338246e0/rpc/src/v1/tests/mocked/eth.rs#L301-L317
  message: '0x0cc175b9c0f1b6a831c399e26977266192eb5ffee6ae2fec3ad71c777531578f',
  signature: '0x9ff8350cc7354b80740a3580d0e0fd4f1f02062040bc06b893d70906f8728bb5163837fd376bf77ce03b55e9bd092b32af60e86abce48f7b8d3539988ee5a9be1c',
  addressHex: '0xbe93f9bacbcffc8ee6663f2647917ed7a20a57bb',
})

signatureTest({
  testLabel: 'sign typed message',
  method: 'eth_signTypedData',
  message: [
    {
      type: 'string',
      name: 'message',
      value: 'Hi, Alice!'
    }
  ],
  signature: '0xb2c9c7bdaee2cc73f318647c3f6e24792fca86a9f2736d9e7537e64c503545392313ebbbcb623c828fd8f99fd1fb48f8f4da8cb1d1a924e28b21de018c826e181c',
  addressHex: '0xbe93f9bacbcffc8ee6663f2647917ed7a20a57bb',
  privateKey: Buffer.from('6969696969696969696969696969696969696969696969696969696969696969', 'hex'),
})

test('sender validation, with mixed-case', function(t){
  t.plan(1)

  var senderAddress = '0xE4660fdAb2D6Bd8b50C029ec79E244d132c3bc2B'

  var providerA = injectMetrics(new HookedWalletTxProvider({
    getAccounts: function(cb){
      cb(null, [senderAddress])
    },
    getPrivateKey: function(address, cb){
      t.pass('correctly validated sender')
      engine.stop()
      t.end()
    },
  }))
  var providerB = injectMetrics(new TestBlockProvider())
  // handle all bottom requests
  var providerC = injectMetrics(new FixtureProvider({
    eth_gasPrice: '0x1234',
    eth_estimateGas: '0x1234',
    eth_getTransactionCount: '0x00',
  }))

  var engine = new ProviderEngine()
  engine.addProvider(providerA)
  engine.addProvider(providerB)
  engine.addProvider(providerC)

  engine.start()
  engine.sendAsync({
    method: 'eth_sendTransaction',
    params: [{
      from: senderAddress.toLowerCase(),
    }]
  }, function(err){
    t.notOk(err, 'error was present')
    engine.stop()
    t.end()
  })

})


function signatureTest({ testLabel, method, privateKey, addressHex, message, signature }) {
  // sign all messages
  var providerA = injectMetrics(new HookedWalletTxProvider({
    getAccounts: function(cb){
      cb(null, [addressHex])
    },
    getPrivateKey: function(address, cb){
      cb(null, privateKey)
    },
  }))

  // handle block requests
  var providerB = injectMetrics(new TestBlockProvider())

  var engine = new ProviderEngine()
  engine.addProvider(providerA)
  engine.addProvider(providerB)

  var payload = {
    method: method,
    params: [message, addressHex],
  }

  singleRpcTest({
    testLabel: `sign message ${method} - ${testLabel}`,
    payload,
    engine,
    expectedResult: signature,
  })

  // Personal sign is supposed to have params
  // ordered in this direction, not the other.
  if (payload.method === 'personal_sign') {
    payload = {
      method: method,
      params: [message, addressHex],
    }

    singleRpcTest({
      testLabel: `sign message ${method} - ${testLabel}`,
      payload,
      engine,
      expectedResult: signature,
    })
  }
}

function recoverTest({ testLabel, method, addressHex, message, signature }) {

  // sign all messages
  var providerA = injectMetrics(new HookedWalletTxProvider({
    getAccounts: function(cb){
      cb(null, [addressHex])
    },
    getPrivateKey: function(address, cb){
      cb(new Error('this should not be called'))
    },
  }))

  // handle block requests
  var blockProvider = injectMetrics(new TestBlockProvider())

  var engine = new ProviderEngine()
  engine.addProvider(providerA)
  engine.addProvider(blockProvider)

  var payload = {
    method: method,
    params: [message, signature],
  }

  singleRpcTest({
    testLabel: `recover message ${method} - ${testLabel}`,
    payload,
    engine,
    expectedResult: addressHex,
  })

}

function singleRpcTest({ testLabel, payload, expectedResult, engine }) {
  test(testLabel, function(t){
    t.plan(3)

    engine.start()
    engine.sendAsync(createPayload(payload), function(err, response){
      if (err) {
        console.log('bad payload:', payload)
        console.error(err)
      }
      t.ifError(err)
      t.ok(response, 'has response')

      t.equal(response.result, expectedResult, 'rpc result is as expected')

      engine.stop()
      t.end()
    })

  })
}
