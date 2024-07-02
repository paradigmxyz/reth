const test = require('tape')
const sigUtil = require('../')

test('normalize address lower cases', function (t) {
  t.plan(1)
  const initial = '0xA06599BD35921CfB5B71B4BE3869740385b0B306'
  const result = sigUtil.normalize(initial)
  t.equal(result, initial.toLowerCase())
})

test('normalize address adds hex prefix', function (t) {
  t.plan(1)
  const initial = 'A06599BD35921CfB5B71B4BE3869740385b0B306'
  const result = sigUtil.normalize(initial)
  t.equal(result, '0x' + initial.toLowerCase())
})

test('normalize an integer converts to byte-pair hex', function (t) {
  t.plan(1)
  const initial = 1
  const result = sigUtil.normalize(initial)
  t.equal(result, '0x01')
})

test('normalize an unsupported type throws', function (t) {
  t.plan(1)
  const initial = {}
  try {
    const result = sigUtil.normalize(initial)
    t.ok(false, 'did not throw')
  } catch (e) {
    t.ok(e, 'should throw')
  }
})

test('personalSign and recover', function (t) {
  t.plan(1)
  const address = '0x29c76e6ad8f28bb1004902578fb108c507be341b'
  console.log('for address ' + address)
  const privKeyHex = '4af1bceebf7f3634ec3cff8a2c38e51178d5d4ce585c52d6043e5e2cc3418bb0'
  const privKey = new Buffer(privKeyHex, 'hex')
  const message = 'Hello, world!'
  const msgParams = { data: message }

  const signed = sigUtil.personalSign(privKey, msgParams)
  msgParams.sig = signed
  const recovered = sigUtil.recoverPersonalSignature(msgParams)

  t.equal(recovered, address)
})

test('personalSign and extractPublicKey', function (t) {
  t.plan(1)
  const privKeyHex = '4af1bceebf7f3634ec3cff8a2c38e51178d5d4ce585c52d6043e5e2cc3418bb0'
  const pubKeyHex = '0x9e9e45b2ec5f070b4e26f57c7fedf647afa7a03e894789816fbd12fedc5acd79d0dfeea925688e177caccb8f5e09f0c289bbcfc7adb98d76f5f8c5259478903a'

  const privKey = new Buffer(privKeyHex, 'hex')
  const message = 'Hello, world!'
  const msgParams = { data: message }

  const signed = sigUtil.personalSign(privKey, msgParams)
  msgParams.sig = signed
  const publicKey = sigUtil.extractPublicKey(msgParams)

  t.equal(publicKey, pubKeyHex)
})

test('signTypedData and recoverTypedSignature - single message', function (t) {
  t.plan(1)
  const address = '0x29c76e6ad8f28bb1004902578fb108c507be341b'
  const privKeyHex = '4af1bceebf7f3634ec3cff8a2c38e51178d5d4ce585c52d6043e5e2cc3418bb0'

  const privKey = Buffer.from(privKeyHex, 'hex')

  const typedData = [
    {
      type: 'string',
      name: 'message',
      value: 'Hi, Alice!'
    }
  ]

  const msgParams = { data: typedData }

  const signature = sigUtil.signTypedData(privKey, msgParams)
  const recovered = sigUtil.recoverTypedSignature({ data: msgParams.data, sig: signature })

  t.equal(address, recovered)
})

test('signTypedData and recoverTypedSignature - multiple messages', function (t) {
  t.plan(1)
  const address = '0x29c76e6ad8f28bb1004902578fb108c507be341b'
  const privKeyHex = '4af1bceebf7f3634ec3cff8a2c38e51178d5d4ce585c52d6043e5e2cc3418bb0'

  const privKey = Buffer.from(privKeyHex, 'hex')

  const typedData = [
    {
      type: 'string',
      name: 'message',
      value: 'Hi, Alice!'
    },
    {
      type: 'uint8',
      name: 'value',
      value: 10
    },
  ]

  const msgParams = { data: typedData }

  const signature = sigUtil.signTypedData(privKey, msgParams)
  const recovered = sigUtil.recoverTypedSignature({ data: msgParams.data, sig: signature })

  t.equal(address, recovered)
})

test('typedSignatureHash - single value', function (t) {
  t.plan(1)
  const typedData = [
    {
      type: 'string',
      name: 'message',
      value: 'Hi, Alice!'
    }
  ]
  const hash = sigUtil.typedSignatureHash(typedData)
  t.equal(hash, '0x14b9f24872e28cc49e72dc104d7380d8e0ba84a3fe2e712704bcac66a5702bd5')
})

test('typedSignatureHash - multiple values', function (t) {
  t.plan(1)
  const typedData = [
    {
      type: 'string',
      name: 'message',
      value: 'Hi, Alice!'
    },
    {
      type: 'uint8',
      name: 'value',
      value: 10
    },
  ]
  const hash = sigUtil.typedSignatureHash(typedData)
  t.equal(hash, '0xf7ad23226db5c1c00ca0ca1468fd49c8f8bbc1489bc1c382de5adc557a69c229')
})

test('typedSignatureHash - bytes', function (t) {
    t.plan(1)
    const typedData = [
        {
            type: 'bytes',
            name: 'message',
            value: '0xdeadbeaf'
        }
    ]
    const hash = sigUtil.typedSignatureHash(typedData)
    t.equal(hash, '0x6c69d03412450b174def7d1e48b3bcbbbd8f51df2e76e2c5b3a5d951125be3a9')
})

typedSignatureHashThrowsTest({
    testLabel: 'empty array',
    argument: []
})

typedSignatureHashThrowsTest({
    testLabel: 'not array',
    argument: 42
})

typedSignatureHashThrowsTest({
    testLabel: 'null',
    argument: null
})

typedSignatureHashThrowsTest({
  testLabel: 'wrong type',
  argument: [
    {
      type: 'jocker',
      name: 'message',
      value: 'Hi, Alice!'
    }
  ]
})

typedSignatureHashThrowsTest({
  testLabel: 'no type',
  argument: [
    {
      name: 'message',
      value: 'Hi, Alice!'
    }
  ]
})

typedSignatureHashThrowsTest({
  testLabel: 'no name',
  argument: [
    {
      type: 'string',
      value: 'Hi, Alice!'
    }
  ]
})

// personal_sign was declared without an explicit set of test data
// so I made a script out of geth's internals to create this test data
// https://gist.github.com/kumavis/461d2c0e9a04ea0818e423bb77e3d260

signatureTest({
  testLabel: 'personalSign - kumavis fml manual test I',
  // "hello world"
  message: '0x68656c6c6f20776f726c64',
  signature: '0xce909e8ea6851bc36c007a0072d0524b07a3ff8d4e623aca4c71ca8e57250c4d0a3fc38fa8fbaaa81ead4b9f6bd03356b6f8bf18bccad167d78891636e1d69561b',
  addressHex: '0xbe93f9bacbcffc8ee6663f2647917ed7a20a57bb',
  privateKey: new Buffer('6969696969696969696969696969696969696969696969696969696969696969', 'hex'),
})

signatureTest({
  testLabel: 'personalSign - kumavis fml manual test II',
  // some random binary message from parity's test
  message: '0x0cc175b9c0f1b6a831c399e26977266192eb5ffee6ae2fec3ad71c777531578f',
  signature: '0x9ff8350cc7354b80740a3580d0e0fd4f1f02062040bc06b893d70906f8728bb5163837fd376bf77ce03b55e9bd092b32af60e86abce48f7b8d3539988ee5a9be1c',
  addressHex: '0xbe93f9bacbcffc8ee6663f2647917ed7a20a57bb',
  privateKey: new Buffer('6969696969696969696969696969696969696969696969696969696969696969', 'hex'),
})

signatureTest({
  testLabel: 'personalSign - kumavis fml manual test III',
  // random binary message data and pk from parity's test
  // https://github.com/ethcore/parity/blob/5369a129ae276d38f3490abb18c5093b338246e0/rpc/src/v1/tests/mocked/eth.rs#L301-L317
  // note: their signature result is incorrect (last byte moved to front) due to a parity bug
  message: '0x0cc175b9c0f1b6a831c399e26977266192eb5ffee6ae2fec3ad71c777531578f',
  signature: '0xa2870db1d0c26ef93c7b72d2a0830fa6b841e0593f7186bc6c7cc317af8cf3a42fda03bd589a49949aa05db83300cdb553116274518dbe9d90c65d0213f4af491b',
  addressHex: '0xe0da1edcea030875cd0f199d96eb70f6ab78faf2',
  privateKey: new Buffer('4545454545454545454545454545454545454545454545454545454545454545', 'hex'),
})

function signatureTest(opts) {
  test(opts.testLabel, function (t) {
    t.plan(2)

    const address = opts.addressHex
    const privKey = opts.privateKey
    const message = opts.message
    const msgParams = { data: message }

    const signed = sigUtil.personalSign(privKey, msgParams)
    t.equal(signed, opts.signature)

    msgParams.sig = signed
    const recovered = sigUtil.recoverPersonalSignature(msgParams)

    t.equal(recovered, address)
  })
}

function typedSignatureHashThrowsTest(opts) {
  const label = `typedSignatureHash - malformed arguments - ${opts.testLabel}`
  test(label, function (t) {
    t.plan(1)

    const argument = opts.argument

    t.throws(() => {
      sigUtil.typedSignatureHash(argument)
    })
  })
}