const EventEmitter = require('events')
const EthQuery = require('eth-query')
const ethUtil = require('ethereumjs-util')

// this is a really minimal shim
// not really tested, i hope it works
// sorry

module.exports = providerEngineSubproviderAsMiddle


function providerEngineSubproviderAsMiddle({ subprovider, provider, blockTracker }) {
  const ethQuery = new EthQuery(provider)
  // create a provider-engine interface
  const engine = new EventEmitter()
  // note: ethQuery fills in omitted params like id
  engine.sendAsync = ethQuery.sendAsync.bind(ethQuery)
  // forward events
  blockTracker.on('sync', engine.emit.bind(engine, 'sync'))
  blockTracker.on('latest', engine.emit.bind(engine, 'latest'))
  blockTracker.on('block', engine.emit.bind(engine, 'rawBlock'))
  blockTracker.on('block', (block) => engine.emit('block', toBufferBlock(block)))
  // set engine
  subprovider.setEngine(engine)

  // create middleware
  return (req, res, next, end) => {
    // send request to subprovider
    subprovider.handleRequest(req, subproviderNext, subproviderEnd)
    // adapter for next handler
    function subproviderNext(nextHandler) {
      if (!nextHandler) return next()
      next((done) => {
        nextHandler(res.error, res.result, done)
      })
    }
    // adapter for end handler
    function subproviderEnd(err, result) {
      if (err) return end(err)
      if (result)
      res.result = result
      end()
    }
  }
}

function toBufferBlock (jsonBlock) {
  return {
    number:           ethUtil.toBuffer(jsonBlock.number),
    hash:             ethUtil.toBuffer(jsonBlock.hash),
    parentHash:       ethUtil.toBuffer(jsonBlock.parentHash),
    nonce:            ethUtil.toBuffer(jsonBlock.nonce),
    sha3Uncles:       ethUtil.toBuffer(jsonBlock.sha3Uncles),
    logsBloom:        ethUtil.toBuffer(jsonBlock.logsBloom),
    transactionsRoot: ethUtil.toBuffer(jsonBlock.transactionsRoot),
    stateRoot:        ethUtil.toBuffer(jsonBlock.stateRoot),
    receiptsRoot:     ethUtil.toBuffer(jsonBlock.receiptRoot || jsonBlock.receiptsRoot),
    miner:            ethUtil.toBuffer(jsonBlock.miner),
    difficulty:       ethUtil.toBuffer(jsonBlock.difficulty),
    totalDifficulty:  ethUtil.toBuffer(jsonBlock.totalDifficulty),
    size:             ethUtil.toBuffer(jsonBlock.size),
    extraData:        ethUtil.toBuffer(jsonBlock.extraData),
    gasLimit:         ethUtil.toBuffer(jsonBlock.gasLimit),
    gasUsed:          ethUtil.toBuffer(jsonBlock.gasUsed),
    timestamp:        ethUtil.toBuffer(jsonBlock.timestamp),
    transactions:     jsonBlock.transactions,
  }
}