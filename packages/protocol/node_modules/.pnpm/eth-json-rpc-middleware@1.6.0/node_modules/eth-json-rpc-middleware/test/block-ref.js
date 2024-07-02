const test = require('tape')
const JsonRpcEngine = require('json-rpc-engine')
const asMiddleware = require('json-rpc-engine/src/asMiddleware')
const RpcBlockTracker = require('eth-block-tracker')
const EthQuery = require('eth-query')
const TestBlockMiddleware = require('eth-block-tracker/test/util/testBlockMiddleware')
const BlockRefMiddleware = require('../block-ref')
const ScaffoldMiddleware = require('../scaffold')

test('contructor - no opts', (t) => {
  t.plan(1)

  t.throws(() => {
    BlockRefMiddleware()
  }, Error, 'Constructor without options fails')
  t.end()
})

test('contructor - empty opts', (t) => {
  t.plan(1)

  t.throws(() => {
    BlockRefMiddleware({})
  }, Error, 'Constructor without empty options')
  t.end()
})

test('provider not ready - shouldnt hang non-"latest" requests', (t) => {
  t.plan(3)

  const { engine, dataEngine, testBlockSource } = createTestSetup()

  // add handler for `test_method`
  dataEngine.push(ScaffoldMiddleware({
    test_method: true
  }))

  // fire request for `test_method`
  engine.handle({ id: 1, method: 'test_method', params: [] }, (err, res) => {
    t.notOk(err, 'No error in response')
    t.ok(res, 'Has response')
    t.equal(res.result, true, 'Response result is correct.')
    t.end()
  })
})

// util

function createTestSetup () {
  // raw data source
  const { engine: dataEngine, testBlockSource } = createEngineForTestData()
  const dataProvider = providerFromEngine(dataEngine)
  // create block tracker
  const blockTracker = new RpcBlockTracker({ provider: dataProvider })
  // create higher level
  const engine = new JsonRpcEngine()
  const provider = providerFromEngine(engine)
  // add block ref middleware
  engine.push(BlockRefMiddleware({ blockTracker }))
  // add data source
  engine.push(asMiddleware(dataEngine))
  const query = new EthQuery(provider)
  return { engine, provider, dataEngine, dataProvider, query, blockTracker, testBlockSource }
}

function createEngineForTestData () {
  const engine = new JsonRpcEngine()
  const testBlockSource = new TestBlockMiddleware()
  engine.push(testBlockSource.createMiddleware())
  return { engine, testBlockSource }
}

function providerFromEngine (engine) {
  const provider = { sendAsync: engine.handle.bind(engine) }
  return provider
}
