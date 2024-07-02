const test = require('tape')
const asyncParallel = require('async/parallel')
const ProviderEngine = require('../index.js')
const FixtureProvider = require('../subproviders/fixture.js')
const InflightCacheProvider = require('../subproviders/inflight-cache.js')
const TestBlockProvider = require('./util/block.js')
const createPayload = require('../util/create-payload.js')
const injectMetrics = require('./util/inject-metrics')

inflightTest('getBalance for latest', {
  method: 'eth_getBalance',
  params: ['0xabcd', 'latest'],
}, true)

inflightTest('getBlock for latest', {
  method: 'eth_getBlockByNumber',
  params: ['latest', false],
}, true)

inflightTest('getBlock for latest then 0', [{
  method: 'eth_getBlockByNumber',
  params: ['latest', false],
}, {
  method: 'eth_getBlockByNumber',
  params: ['0x0', false],
}], false)

function inflightTest(label, payloads, shouldHitCacheOnSecondRequest){
  if (!Array.isArray(payloads)) {
    payloads = [payloads, payloads]
  }

  test('inflight cache - '+label, function(t){
    t.plan(6)

    // cache layer
    var cacheProvider = injectMetrics(new InflightCacheProvider())
    // handle balance
    var dataProvider = injectMetrics(new FixtureProvider({
      eth_getBalance: '0xdeadbeef',
    }))
    // handle dummy block
    var blockProvider = injectMetrics(new TestBlockProvider())

    var engine = new ProviderEngine()
    engine.addProvider(cacheProvider)
    engine.addProvider(dataProvider)
    engine.addProvider(blockProvider)

    // run polling until first block
    engine.start()
    engine.once('block', () => {
      // stop polling
      engine.stop()
      // clear subprovider metrics
      cacheProvider.clearMetrics()
      dataProvider.clearMetrics()
      blockProvider.clearMetrics()

      // determine which provider will handle the request
      const isBlockTest = (payloads[0].method === 'eth_getBlockByNumber')
      const handlingProvider = isBlockTest ? blockProvider : dataProvider

      // begin cache test
      cacheCheck(t, engine, cacheProvider, handlingProvider, payloads, function(err, response) {
        t.end()
      })
    })

    function cacheCheck(t, engine, cacheProvider, handlingProvider, payloads, cb) {
      var method = payloads[0].method
      requestSimultaneous(payloads, noop, noop, function(err, responses){
        // first request
        t.ifError(err, 'did not error')
        t.ok(responses && responses.filter(Boolean).length, 'has responses')

        if (shouldHitCacheOnSecondRequest) {

          t.equal(cacheProvider.getWitnessed(method).length, 2, 'cacheProvider did see "'+method+'"')
          t.equal(cacheProvider.getHandled(method).length, 1, 'cacheProvider did NOT handle "'+method+'"')

          t.equal(handlingProvider.getWitnessed(method).length, 1, 'handlingProvider did see "'+method+'"')
          t.equal(handlingProvider.getHandled(method).length, 1, 'handlingProvider did handle "'+method+'"')
        
        } else {

          t.equal(cacheProvider.getWitnessed(method).length, 2, 'cacheProvider did see "'+method+'"')
          t.equal(cacheProvider.getHandled(method).length, 0, 'cacheProvider did NOT handle "'+method+'"')

          t.equal(handlingProvider.getWitnessed(method).length, 2, 'handlingProvider did see "'+method+'"')
          t.equal(handlingProvider.getHandled(method).length, 2, 'handlingProvider did handle "'+method+'"')

        }

      })
    }

    function requestSimultaneous(payloads, afterFirst, afterSecond, cb){
      asyncParallel([
        (cb) => {
          engine.sendAsync(createPayload(payloads[0]), (err, result) => {
            afterFirst(err, result)
            cb(err, result)
          })
        },
        (cb) => {
          engine.sendAsync(createPayload(payloads[1]), (err, result) => {
            afterSecond(err, result)
            cb(err, result)
          })
        },
      ], cb)
    }
  })

}

function noop(){}