const test = require('tape')
const ProviderEngine = require('../index.js')
const SubscriptionSubprovider = require('../subproviders/subscriptions.js')
const TestBlockProvider = require('./util/block.js')
const createPayload = require('../util/create-payload.js')
const injectMetrics = require('./util/inject-metrics')

subscriptionTest('basic block subscription', {
    method: 'eth_subscribe',
    params: ['newHeads']
  },
  function afterInstall(t, testMeta, response, cb){
    // nothing to do here, we just need a new block, which subscriptionTest does for us
    cb()
  },
  function subscriptionChanges(t, testMeta, response, cb){
    let returnedBlockHash = response.params.result.hash
    t.equal(returnedBlockHash, testMeta.block.hash, 'correct result')
    cb()
  }
)

subscriptionTest('log subscription - basic', {
    method: 'eth_subscribe',
    params: ['logs', {
      topics: ['0x00000000000000000000000000000000000000000000000000deadbeefcafe01']
    }],
  },
  function afterInstall(t, testMeta, response, cb){
    testMeta.tx = testMeta.blockProvider.addTx({
      topics: ['0x00000000000000000000000000000000000000000000000000deadbeefcafe01']
    })
    testMeta.badTx = testMeta.blockProvider.addTx({
      topics: ['0x00000000000000000000000000000000000000000000000000deadbeefcafe02']
    })
    cb()
  },
  function subscriptionChanges(t, testMeta, response, cb){
    var matchedTx = response.params.result
    t.equal(matchedTx, testMeta.tx, 'correct result')
    cb()
  }
)

subscriptionTest('log subscription - and logic', {
    method: 'eth_subscribe',
    params: ['logs', {
      topics: [
      '0x00000000000000000000000000000000000000000000000000deadbeefcafe01',
      '0x00000000000000000000000000000000000000000000000000deadbeefcafe02',
      ],
    }],
  },
  function afterInstall(t, testMeta, response, cb){
    testMeta.tx = testMeta.blockProvider.addTx({
      topics: [
        '0x00000000000000000000000000000000000000000000000000deadbeefcafe01',
        '0x00000000000000000000000000000000000000000000000000deadbeefcafe02',
      ],
    })
    testMeta.badTx = testMeta.blockProvider.addTx({
      topics: [
        '0x00000000000000000000000000000000000000000000000000deadbeefcafe02',
        '0x00000000000000000000000000000000000000000000000000deadbeefcafe01',
      ],
    })
    cb()
  },
  function subscriptionChangesOne(t, testMeta, response, cb){
    var matchedTx = response.params.result
    t.equal(matchedTx, testMeta.tx, 'correct result')
    cb()
  }
)

subscriptionTest('log subscription - or logic', {
    method: 'eth_subscribe',
    params: ['logs', {
      topics: [
        [
          '0x00000000000000000000000000000000000000000000000000deadbeefcafe01',
          '0x00000000000000000000000000000000000000000000000000deadbeefcafe02',
        ],
      ],
    }],
  },
  function afterInstall(t, testMeta, response, cb){
    testMeta.tx1 = testMeta.blockProvider.addTx({
      topics: [
        '0x00000000000000000000000000000000000000000000000000deadbeefcafe01',
      ],
    })
    cb()
  },
  function subscriptionChangesOne(t, testMeta, response, cb){
    var matchedTx1 = response.params.result
    t.equal(matchedTx1, testMeta.tx1, 'correct result')

    testMeta.tx2 = testMeta.blockProvider.addTx({
      topics: [
        '0x00000000000000000000000000000000000000000000000000deadbeefcafe02',
      ],
    })
    cb()
  },
  function subscriptionChangesTwo(t, testMeta, response, cb){
    var matchedTx2 = response.params.result
    t.equal(matchedTx2, testMeta.tx2, 'correct result')
    cb()
  }
)

subscriptionTest('log subscription - wildcard logic', {
    method: 'eth_subscribe',
    params: ['logs', {
      topics: [
        null,
        '0x00000000000000000000000000000000000000000000000000deadbeefcafe02',
      ],
    }],
  },
  function afterInstall(t, testMeta, response, cb){
    testMeta.tx1 = testMeta.blockProvider.addTx({
      topics: [
        '0x00000000000000000000000000000000000000000000000000deadbeefcafe01',
        '0x00000000000000000000000000000000000000000000000000deadbeefcafe02',
      ],
    })
    cb()
  },
  function subscriptionChangesOne(t, testMeta, response, cb){
    var matchedTx1 = response.params.result
    t.equal(matchedTx1, testMeta.tx1, 'correct result')
    testMeta.tx2 = testMeta.blockProvider.addTx({
      topics: [
        '0x00000000000000000000000000000000000000000000000000deadbeefcafe02',
        '0x00000000000000000000000000000000000000000000000000deadbeefcafe02',
      ],
    })
    cb()
  },
  function subscriptionChangesTwo(t, testMeta, response, cb){
    var matchedTx2 = response.params.result
    t.equal(matchedTx2, testMeta.tx2, 'correct result')
    cb()
  }
)

subscriptionTest('block subscription - parsing large difficulty', {
    method: 'eth_subscribe',
    params: ['newHeads']
  },
  function afterInstall(t, testMeta, response, cb) {
    testMeta.blockProvider.nextBlock({
      gasLimit: '0x01',
      difficulty: '0xfffffffffffffffffffffffffffffffe'
    })
    cb()
  },
  function subscriptionChangesOne(t, testMeta, response, cb) {
    var returnedDifficulty = response.params.result.difficulty
    var returnedGasLimit = response.params.result.gasLimit
    t.equal(returnedDifficulty, '0xfffffffffffffffffffffffffffffffe', 'correct result')
    t.equal(returnedGasLimit, '0x1', 'correct result')
    cb()
  }
)

function subscriptionTest(label, subscriptionPayload, afterInstall, subscriptionChangesOne, subscriptionChangesTwo) {
  let testMeta = {}
  let t = test('subscriptions - '+label, function(t) {
    // subscribe
    // new block
    // check for notification


    // handle "test_rpc"
    let subscriptionSubprovider = testMeta.subscriptionSubprovider = injectMetrics(new SubscriptionSubprovider())
    // handle block requests
    let blockProvider = testMeta.blockProvider = injectMetrics(new TestBlockProvider())

    let engine = testMeta.engine = new ProviderEngine({
      pollingInterval: 20,
      pollingShouldUnref: false,
    })
    engine.addProvider(subscriptionSubprovider)
    engine.addProvider(blockProvider)
    engine.once('block', startTest)

    setTimeout(() => {
      engine.start()
    }, 1)

    function startTest(){
      // register subscription
      engine.sendAsync(createPayload(subscriptionPayload), function(err, response){
        t.ifError(err, 'did not error')
        t.ok(response, 'has response')

        let method = subscriptionPayload.method

        t.equal(subscriptionSubprovider.getWitnessed(method).length, 1, 'subscriptionSubprovider did see "'+method+'"')
        t.equal(subscriptionSubprovider.getHandled(method).length, 1, 'subscriptionSubprovider did handle "'+method+'"')

        let subscriptionId = testMeta.subscriptionId = response.result

        // manipulates next block to trigger a notification
        afterInstall(t, testMeta, response, function(err){
          t.ifError(err, 'did not error')
          subscriptionSubprovider.once('data', continueTest)
          // create next block so that notification is sent
          testMeta.block = testMeta.blockProvider.nextBlock()
        })
      })
    }

    // handle first notification
    function continueTest(err, notification){
      let subscriptionId = testMeta.subscriptionId
      // after subscription check one
      t.ifError(err, 'did not error')
      t.ok(notification, 'has notification')
      t.equal(notification.params.subscription, subscriptionId, 'notification has correct subscription id')

      // test-specific checks, and make changes to next block to trigger next notification
      subscriptionChangesOne(t, testMeta, notification, function(err){
        t.ifError(err, 'did not error')

        if (subscriptionChangesTwo) {
          subscriptionSubprovider.once('data', function (err, notification) {
            t.ifError(err, 'did not error')
            t.ok(notification, 'has notification')

            // final checks
            subscriptionChangesTwo(t, testMeta, notification, function (err) {
              t.ifError(err, 'did not error')
              end()
            })
          })

          // trigger a new block so that the above handler runs
          testMeta.block = testMeta.blockProvider.nextBlock()
        } else {
          end()
        }
      })
    }

    function end() {
      engine.sendAsync(createPayload({ method: 'eth_unsubscribe', params: [testMeta.subscriptionId] }), function (err, response) {
        testMeta.engine.stop()
        t.ifError(err, 'did not error')
        t.end()
      })
    }
  })
}
