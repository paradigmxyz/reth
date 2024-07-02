const test = require('tape')
const JsonRpcEngine = require('json-rpc-engine')
const asMiddleware = require('json-rpc-engine/src/asMiddleware')
const RpcBlockTracker = require('eth-block-tracker')
const EthQuery = require('eth-query')
const TestBlockMiddleware = require('eth-block-tracker/test/util/testBlockMiddleware')
const createInflightCacheMiddleware = require('../inflight-cache')
const createScaffoldMiddleware = require('../scaffold')

test('inflight-cache - basic', (t) => {
  t.plan(10)

  const { engine } = createTestSetup()

  let releaseStall = null
  let hitCount = 0
  let res1, res2

  // add inflight cache
  engine.push(createInflightCacheMiddleware())

  // add stalling result handler for `test_blockCache`
  engine.push((req, res, next, end) => {
    hitCount++
    res.result = true
    releaseStall = end
  })

  // fire first request (handled but stalled)
  engine.handle({ id: 1, method: 'test_blockCache', params: [] }, firstReqResponse)
  // fire second request (inflight cached)
  engine.handle({ id: 2, method: 'test_blockCache', params: [] }, secondReqResponse)

  t.ok(releaseStall, 'test prep - releaseStall was set')
  t.equal(hitCount, 1, 'test prep - result handler was only hit once')

  // release stalled first request
  releaseStall()

  function firstReqResponse (err, res) {
    res1 = res
    t.notOk(err, 'No error in response')
    t.ok(res, 'Has response')
    t.equal(res.result, true, 'Response result is correct.')
  }

  function secondReqResponse (err, res) {
    res2 = res
    t.notOk(err, 'No error in response')
    t.ok(res, 'Has response')
    t.equal(res.result, true, 'Response result is correct.')
    t.equal(hitCount, 1, 'result handler was only hit once')
    t.notEqual(res1, res2, 'response objects were unique')
    t.end()
  }
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
  // engine.push(BlockRefMiddleware({ blockTracker }))
  // add data source
  // engine.push(asMiddleware(dataEngine))
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
