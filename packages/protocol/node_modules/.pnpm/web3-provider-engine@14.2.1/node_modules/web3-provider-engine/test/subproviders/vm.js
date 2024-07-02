const test = require('tape')
const async = require('async')
const ethUtil = require('ethereumjs-util')
const ProviderEngine = require('../../index.js')
const VmSubprovider = require('../../subproviders/vm')
const TestBlockProvider = require('../util/block.js')
const RpcSubprovider = require('../../subproviders/rpc')
const createPayload = require('../../util/create-payload.js')
const rpcHexEncoding = require('../../util/rpc-hex-encoding.js')

test('binary search eth_estimateGas implementation', function(t) {
    var gasNeededScenarios = [
        {
            gasNeeded: 5,
            gasEstimate: 1150,
            numIterations: 12,
        },
        {
            gasNeeded: 50000,
            gasEstimate: 50046,
            numIterations: 13,
        },
        {
            gasNeeded: 4712387,
            gasEstimate: 4712387,
            numIterations: 23, // worst-case scenario
        },
    ]

    async.eachSeries(gasNeededScenarios, function(scenario, next) {
        var engine = new ProviderEngine()
        var vmSubprovider = new VmSubprovider()
        var numIterations = 0
        // Stub runVm so that it behaves as if it needs gasNeeded to run and increments numIterations
        vmSubprovider.runVm = function(payload, cb) {
            numIterations++
            if (payload.params[0].gas < scenario.gasNeeded) {
                cb(new Error('fake out of gas'))
            } else {
                cb(null, {
                    gasUsed: ethUtil.toBuffer(scenario.gasNeeded),
                });
            }
        }
        engine.addProvider(vmSubprovider)
        engine.addProvider(new TestBlockProvider());
        engine.start()
        engine.sendAsync(createPayload({
          method: 'eth_estimateGas',
          params: [{}, 'latest'],
        }), function(err, response) {
          t.ifError(err, 'did not error')
          t.ok(response, 'has response')

          var gasEstimationInt = rpcHexEncoding.quantityHexToInt(response.result)
          t.equal(gasEstimationInt, scenario.gasEstimate, 'properly calculates gas needed')
          t.equal(numIterations, scenario.numIterations, 'ran expected number of iterations')

          engine.stop()
          next()
        })
    }, function(err) {
        t.ifError(err, 'did not error')
        t.end()
    })
})
