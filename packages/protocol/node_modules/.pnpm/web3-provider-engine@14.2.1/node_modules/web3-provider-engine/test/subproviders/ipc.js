const sha3 = require('ethereumjs-util').sha3;
const test = require('tape')
const ProviderEngine = require('../../index.js')
const createPayload = require('../../util/create-payload.js')
const IpcSubprovider = require('../../subproviders/ipc')
const socketPath = process.argv[2]; // e.g. '/root/.ethereum/geth.ipc'

test('ipc personal_listAccounts', function(t) {
  t.plan(3)
  var engine = new ProviderEngine()
  var ipc = new IpcSubprovider({ipcPath : socketPath});
  engine.addProvider(ipc)
  engine.start()
  engine.sendAsync(createPayload({
    method: 'personal_listAccounts',
    params: [],
  }), function(err, response){
    t.ifError(err, 'throw no error')
    t.ok(response, 'has response')
    t.equal(typeof response.result[0], 'string')
    t.end()
  })
})

test('ipc personal_newAccount', function(t) {
  t.plan(3)
  var engine = new ProviderEngine()
  var ipc = new IpcSubprovider({ipcPath : socketPath});
  engine.addProvider(ipc)
  engine.start()
  engine.sendAsync(createPayload({
    method: 'personal_newAccount',
    params: ['test'],
  }), function(err, response){
    t.ifError(err, 'throw no error')
    t.ok(response, 'has response')
    t.equal(response.result.length, 42);
    t.end()
  })
})
