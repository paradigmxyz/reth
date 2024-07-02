const test = require('tape')
const ProviderEngine = require('../../index.js')
const createPayload = require('../../util/create-payload.js')
const FixtureProvider = require('../../subproviders/fixture.js')
const SanitizerSubprovider = require('../../subproviders/sanitizer')
const MockSubprovider = require('../util/mock-subprovider')
const TestBlockProvider = require('../util/block.js')
const extend = require('xtend')

test('Sanitizer removes unknown keys', function(t) {
  t.plan(8)

  var engine = new ProviderEngine()

  var sanitizer = new SanitizerSubprovider()
  engine.addProvider(sanitizer)

  // test sanitization
  var checkSanitizer = new FixtureProvider({
    test_unsanitized: (req, next, end) => {
      if (req.method !== 'test_unsanitized') return next()
      const firstParam = payload.params[0]
      t.notOk(firstParam && firstParam.foo)
      t.equal(firstParam.gas, '0x01')
      t.equal(firstParam.data, '0x01')
      t.equal(firstParam.fromBlock, 'latest')
      t.equal(firstParam.topics.length, 3)
      t.equal(firstParam.topics[1], '0x0a')
      end(null, { baz: 'bam' })
    },
  })
  engine.addProvider(checkSanitizer)

  // handle block requests
  var blockProvider = new TestBlockProvider()
  engine.addProvider(blockProvider)

  engine.start()

  var payload = {
    method: 'test_unsanitized',
    params: [{
      foo: 'bar',
      gas: '0x01',
      data: '01',
      fromBlock: 'latest',
      topics: [
        null,
        '0X0A',
        '0x03',
      ],
    }],
  }
  engine.sendAsync(payload, function (err, response) {
    engine.stop()
    t.notOk(err, 'no error')
    t.equal(response.result.baz, 'bam', 'result was received correctly')
    t.end()
  })
})