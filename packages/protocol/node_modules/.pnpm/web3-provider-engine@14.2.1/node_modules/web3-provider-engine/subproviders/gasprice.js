/*
 * Calculate gasPrice based on last blocks.
 * @author github.com/axic
 *
 * FIXME: support minimum suggested gas and perhaps other options from geth:
 * https://github.com/ethereum/go-ethereum/blob/master/eth/gasprice.go
 * https://github.com/ethereum/go-ethereum/wiki/Gas-Price-Oracle
 */

const map = require('async/map')
const inherits = require('util').inherits
const Subprovider = require('./subprovider.js')

module.exports = GaspriceProvider

inherits(GaspriceProvider, Subprovider)

function GaspriceProvider(opts) {
  opts = opts || {}
  this.numberOfBlocks = opts.numberOfBlocks || 10
  this.delayInBlocks = opts.delayInBlocks || 5
}

GaspriceProvider.prototype.handleRequest = function(payload, next, end){
  if (payload.method !== 'eth_gasPrice')
    return next()

  const self = this

  self.emitPayload({ method: 'eth_blockNumber' }, function(err, res) {
    // FIXME: convert number using a bignum library
    var lastBlock = parseInt(res.result, 16) - self.delayInBlocks
    var blockNumbers = [ ]
    for (var i = 0; i < self.numberOfBlocks; i++) {
      blockNumbers.push('0x' + lastBlock.toString(16))
      lastBlock--
    }

    function getBlock(item, end) {
      self.emitPayload({ method: 'eth_getBlockByNumber', params: [ item, true ] }, function(err, res) {
        if (err) return end(err)
        if (!res.result) return end(new Error(`GaspriceProvider - No block for "${item}"`))
        end(null, res.result.transactions)
      })
    }

    // FIXME: this could be made much faster
    function calcPrice(err, transactions) {
      // flatten array
      transactions = transactions.reduce(function(a, b) { return a.concat(b) }, [])

      // leave only the gasprice
      // FIXME: convert number using a bignum library
      transactions = transactions.map(function(a) { return parseInt(a.gasPrice, 16) }, [])

      // order ascending
      transactions.sort(function(a, b) { return a - b })

      // ze median
      var half = Math.floor(transactions.length / 2)

      var median
      if (transactions.length % 2)
        median = transactions[half]
      else
        median = Math.floor((transactions[half - 1] + transactions[half]) / 2.0)

      end(null, median)
    }

    map(blockNumbers, getBlock, calcPrice)
  })
}
