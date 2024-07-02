const test = require('tape')
const ProviderEngine = require('../index.js')
const FixtureProvider = require('../subproviders/fixture.js')
const CacheProvider = require('../subproviders/cache.js')
const TestBlockProvider = require('./util/block.js')
const createPayload = require('../util/create-payload.js')
const injectMetrics = require('./util/inject-metrics')

// skip cache

cacheTest('skipCache - true', {
  method: 'eth_getBalance',
  skipCache: true,
}, false)

cacheTest('skipCache - false', {
  method: 'eth_getBalance',
  skipCache: false,
}, true)

// block tags

cacheTest('getBalance + undefined blockTag', {
  method: 'eth_getBalance',
  params: ['0x1234'],
}, true)

cacheTest('getBalance + latest blockTag', {
  method: 'eth_getBalance',
  params: ['0x1234', 'latest'],
}, true)

cacheTest('getBalance + pending blockTag', {
  method: 'eth_getBalance',
  params: ['0x1234', 'pending'],
}, false)

// tx by hash

cacheTest('getTransactionByHash for transaction that doesn\'t exist', {
  method: 'eth_getTransactionByHash',
  params: ['0x00000000000000000000000000000000000000000000000000deadbeefcafe00'],
}, false)

cacheTest('getTransactionByHash for transaction that\'s pending', {
  method: 'eth_getTransactionByHash',
  params: ['0x00000000000000000000000000000000000000000000000000deadbeefcafe01'],
}, false)

cacheTest('getTransactionByHash for mined transaction', {
  method: 'eth_getTransactionByHash',
  params: ['0x00000000000000000000000000000000000000000000000000deadbeefcafe02'],
}, true)

// code

cacheTest('getCode for latest block, then for earliest block, should not return cached response on second request', [{
  method: 'eth_getCode',
  params: ['0x1234', 'latest'],
}, {
  method: 'eth_getCode',
  params: ['0x1234', 'earliest'],
}], false)

cacheTest('getCode for a specific block, then for the one before it, should not return cached response on second request', [{
  method: 'eth_getCode',
  params: ['0x1234', '0x3'],
}, {
  method: 'eth_getCode',
  params: ['0x1234', '0x2'],
}], false)

cacheTest('getCode for a specific block, then the one after it, should return cached response on second request', [{
  method: 'eth_getCode',
  params: ['0x1234', '0x2'],
}, {
  method: 'eth_getCode',
  params: ['0x1234', '0x3'],
}], true)

cacheTest('getCode for an unspecified block, then for the latest, should return cached response on second request', [{
  method: 'eth_getCode',
  params: ['0x1234'],
}, {
  method: 'eth_getCode',
  params: ['0x1234', 'latest'],
}], true)

// blocks

cacheTest('getBlockForNumber for latest then block 0', [{
  method: 'eth_getBlockByNumber',
  params: ['latest'],
}, {
  method: 'eth_getBlockByNumber',
  params: ['0x0'],
}], false)

cacheTest('getBlockForNumber for latest then block 1', [{
  method: 'eth_getBlockByNumber',
  params: ['latest'],
}, {
  method: 'eth_getBlockByNumber',
  params: ['0x1'],
}], false)

cacheTest('getBlockForNumber for 0 then block 1', [{
  method: 'eth_getBlockByNumber',
  params: ['0x0'],
}, {
  method: 'eth_getBlockByNumber',
  params: ['0x1'],
}], false)

cacheTest('getBlockForNumber for block 0', [{
  method: 'eth_getBlockByNumber',
  params: ['0x0'],
}, {
  method: 'eth_getBlockByNumber',
  params: ['0x0'],
}], true)

cacheTest('getBlockForNumber for block 1', [{
  method: 'eth_getBlockByNumber',
  params: ['0x1'],
}, {
  method: 'eth_getBlockByNumber',
  params: ['0x1'],
}], true)

// storage

cacheTest('getStorageAt for same block should cache', [{
  method: 'eth_getStorageAt',
  params: ['0x295a70b2de5e3953354a6a8344e616ed314d7251', '0x0', '0x1234'],
}, {
  method: 'eth_getStorageAt',
  params: ['0x295a70b2de5e3953354a6a8344e616ed314d7251', '0x0', '0x1234'],
}], true)

cacheTest('getStorageAt for different block should not cache', [{
  method: 'eth_getStorageAt',
  params: ['0x295a70b2de5e3953354a6a8344e616ed314d7251', '0x0', '0x1234'],
}, {
  method: 'eth_getStorageAt',
  params: ['0x295a70b2de5e3953354a6a8344e616ed314d7251', '0x0', '0x4321'],
}], false)


// test helper for caching
// 1. Sets up caching and data provider
// 2. Performs first request
// 3. Performs second request
// 4. checks if cache hit or missed

function cacheTest(label, payloads, shouldHitCacheOnSecondRequest){
  if (!Array.isArray(payloads)) {
    payloads = [payloads, payloads]
  }

  test('cache - '+label, function(t){
    t.plan(12)

    // cache layer
    var cacheProvider = injectMetrics(new CacheProvider())
    // handle balance
    var dataProvider = injectMetrics(new FixtureProvider({
      eth_getBalance: '0xdeadbeef',
      eth_getCode: '6060604052600560005560408060156000396000f3606060405260e060020a60003504633fa4f245811460245780635524107714602c575b005b603660005481565b6004356000556022565b6060908152602090f3',
      eth_getTransactionByHash: function(payload, next, end) {
        // represents a pending tx
        if (payload.params[0] === '0x00000000000000000000000000000000000000000000000000deadbeefcafe00') {
          end(null, null)
        } else if (payload.params[0] === '0x00000000000000000000000000000000000000000000000000deadbeefcafe01') {
          end(null, {
            hash: '0x00000000000000000000000000000000000000000000000000deadbeefcafe01',
            nonce: '0xd',
            blockHash: null,
            blockNumber: null,
            transactionIndex: null,
            from: '0xb1cc05ab12928297911695b55ee78c1188f8ef91',
            to: '0xfbb1b73c4f0bda4f67dca266ce6ef42f520fbb98',
            value: '0xddb66b2addf4800',
            gas: '0x5622',
            gasPrice: '0xba43b7400',
            input: '0x',
          })
        } else {
          end(null, {
            hash: payload.params[0],
            nonce: '0xd',
            blockHash: '0x1',
            blockNumber: '0x1',
            transactionIndex: '0x0',
            from: '0xb1cc05ab12928297911695b55ee78c1188f8ef91',
            to: '0xfbb1b73c4f0bda4f67dca266ce6ef42f520fbb98',
            value: '0xddb66b2addf4800',
            gas: '0x5622',
            gasPrice: '0xba43b7400',
            input: '0x',
          })
        }
      },
      eth_getStorageAt: '0x00000000000000000000000000000000000000000000000000000000deadbeef',
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
      requestTwice(payloads, function(err, response){
        // first request
        t.ifError(err || response.error && response.error.message, 'did not error')
        t.ok(response, 'has response')

        t.equal(cacheProvider.getWitnessed(method).length, 1, 'cacheProvider did see "'+method+'"')
        t.equal(cacheProvider.getHandled(method).length, 0, 'cacheProvider did NOT handle "'+method+'"')

        t.equal(handlingProvider.getWitnessed(method).length, 1, 'handlingProvider did see "'+method+'"')
        t.equal(handlingProvider.getHandled(method).length, 1, 'handlingProvider did handle "'+method+'"')

      }, function(err, response){
        // second request
        t.ifError(err || response.error && response.error.message, 'did not error')
        t.ok(response, 'has response')

        if (shouldHitCacheOnSecondRequest) {
          t.equal(cacheProvider.getWitnessed(method).length, 2, 'cacheProvider did see "'+method+'"')
          t.equal(cacheProvider.getHandled(method).length, 1, 'cacheProvider did handle "'+method+'"')

          t.equal(handlingProvider.getWitnessed(method).length, 1, 'handlingProvider did NOT see "'+method+'"')
          t.equal(handlingProvider.getHandled(method).length, 1, 'handlingProvider did NOT handle "'+method+'"')
        } else {
          t.equal(cacheProvider.getWitnessed(method).length, 2, 'cacheProvider did see "'+method+'"')
          t.equal(cacheProvider.getHandled(method).length, 0, 'cacheProvider did handle "'+method+'"')

          t.equal(handlingProvider.getWitnessed(method).length, 2, 'handlingProvider did NOT see "'+method+'"')
          t.equal(handlingProvider.getHandled(method).length, 2, 'handlingProvider did NOT handle "'+method+'"')
        }

        cb()
      })
    }

    function requestTwice(payloads, afterFirst, afterSecond){
      engine.sendAsync(createPayload(payloads[0]), function(err, result){
        afterFirst(err, result)
        engine.sendAsync(createPayload(payloads[1]), afterSecond)
      })
    }

  })

}
