const xhr = process.browser ? require('xhr') : require('request')
const onStreamEnd = require('end-of-stream')
const createZeroClient = require('web3-provider-engine/zero')
const jsonSerialize = require('json-stable-stringify')
const createVmTraceStream = require('./index.js').createVmTraceStream
const createCallTraceTransform = require('./call-trace')
const traceTransaction = require('./trace-transaction')

const RPC_ENDPOINT = 'https://mainnet.infura.io/'
// const RPC_ENDPOINT = 'http://localhost:8545'
// long tx run
// const targetTx = '0x44ddb2dc10f0354ba87814a17e58765b7bf1a7d47baa2fac9cf5b72f462c66cd'
// lots of setup + long tx run
// const targetTx = '0x9f004c8acac4457d985154a1004e0b43c9c8010697abfc3796de84ca81b93d05'
// invalid jump
// const targetTx = '0x026084424ed68542b611f8deffb2563bb527600abf63cee61d1cd8850f1b94fe'
// DAO getting ripped
// const targetTx = '0xc0b6d5916bff007ef3a349b9191300e210a5fbb1db7f1cece50184c479947bc3'
// MKR transfer
const targetTx = '0xb6c8ae5933c9b5a3e8a732ecb2530358695c843d807c47bacf2263043c3966eb'
// multisig confirm
// const targetTx = '0x0b68379d9cbac4e345bb4e222760f73b905cdfe3887c56df95c89b65e88e46c6'

const provider = createZeroClient({
  rpcUrl: RPC_ENDPOINT,
  getAccounts: (cb) => cb(null, []),
})
// log network requests
// _sendAsync = provider.sendAsync.bind(provider)
// provider.sendAsync = function(payload, cb){ _sendAsync(payload, function(err, res){ console.log(payload, '->', res); cb.apply(null, arguments) }) }

tryVmStream()
// tryCallTraceStream()
// tryTrace()

function tryVmStream(){
  const vmStream = createVmTraceStream(provider, targetTx)
  vmStream.on('data', console.log)
  onStreamEnd(vmStream, (err) => {
    if (err) console.error(err)
    provider.stop()
  })
}

function tryCallTraceStream(){
  const vmStream = createVmTraceStream(provider, targetTx)
  const callTraceTransform = createCallTraceTransform()
  vmStream.pipe(callTraceTransform)
  callTraceTransform.on('data', console.log)
  onStreamEnd(callTraceTransform, (err) => {
    if (err) console.error(err)
    provider.stop()
  })
}

function tryTrace(){
  traceTransaction(provider, targetTx, (err, result) => {
    provider.stop()
    if (err) return console.error(err)
    console.log(jsonSerialize(result, { space: 2 }))
  })
}
