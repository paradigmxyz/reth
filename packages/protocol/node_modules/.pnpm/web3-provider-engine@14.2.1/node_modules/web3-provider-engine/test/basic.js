const test = require('tape')
const ProviderEngine = require('../index.js')
const PassthroughProvider = require('./util/passthrough.js')
const FixtureProvider = require('../subproviders/fixture.js')
const TestBlockProvider = require('./util/block.js')
const createPayload = require('../util/create-payload.js')
const injectMetrics = require('./util/inject-metrics')


test('fallthrough test', function(t){
  t.plan(8)

  // handle nothing
  var providerA = injectMetrics(new PassthroughProvider())
  // handle "test_rpc"
  var providerB = injectMetrics(new FixtureProvider({
    test_rpc: true,
  }))
  // handle block requests
  var providerC = injectMetrics(new TestBlockProvider())

  var engine = new ProviderEngine()
  engine.addProvider(providerA)
  engine.addProvider(providerB)
  engine.addProvider(providerC)

  engine.start()
  engine.sendAsync(createPayload({ method: 'test_rpc' }), function(err, response){
    t.ifError(err, 'did not error')
    t.ok(response, 'has response')

    t.equal(providerA.getWitnessed('test_rpc').length, 1, 'providerA did see "test_rpc"')
    t.equal(providerA.getHandled('test_rpc').length, 0, 'providerA did NOT handle "test_rpc"')

    t.equal(providerB.getWitnessed('test_rpc').length, 1, 'providerB did see "test_rpc"')
    t.equal(providerB.getHandled('test_rpc').length, 1, 'providerB did handle "test_rpc"')

    t.equal(providerC.getWitnessed('test_rpc').length, 0, 'providerC did NOT see "test_rpc"')
    t.equal(providerC.getHandled('test_rpc').length, 0, 'providerC did NOT handle "test_rpc"')

    engine.stop()
    t.end()
  })

})

test('add provider at index', function(t){
  var providerA = new PassthroughProvider()
  var providerB = new PassthroughProvider()
  var providerC = new PassthroughProvider()
  var engine = new ProviderEngine()
  engine.addProvider(providerA)
  engine.addProvider(providerB)
  engine.addProvider(providerC, 1)

  t.deepEqual(engine._providers, [providerA, providerC, providerB])
  t.end()
})

test('remove provider', function(t){
  var providerA = new PassthroughProvider()
  var providerB = new PassthroughProvider()
  var providerC = new PassthroughProvider()
  var engine = new ProviderEngine()
  engine.addProvider(providerA)
  engine.addProvider(providerB)
  engine.addProvider(providerC)
  engine.removeProvider(providerB)

  t.deepEqual(engine._providers, [providerA, providerC])
  t.end()
})
