const inherits = require('util').inherits
const extend = require('xtend')
const ethUtil = require('ethereumjs-util')
const FixtureProvider = require('../../subproviders/fixture.js')

module.exports = TestBlockProvider

//
// handles only `eth_getBlockByNumber` requests
// returns a dummy block
//

inherits(TestBlockProvider, FixtureProvider)
function TestBlockProvider(methods){
  const self = this
  self._blockChain = {}
  self._pendingTxs = []
  self.nextBlock()
  FixtureProvider.call(self, {
    eth_getBlockByNumber: function(payload, next, end){
      const blockRef = payload.params[0]
      const result = self.getBlockByRef(blockRef)
      // return result asynchronously
      setTimeout(() => end(null, result))
    },
    eth_getLogs: function(payload, next, end){
      const transactions = self._currentBlock.transactions
      // return result asynchronously
      setTimeout(() => end(null, transactions))
    },
  })
}

// class _currentBlocks
TestBlockProvider.createBlock = createBlock
TestBlockProvider.incrementHex = incrementHex

TestBlockProvider.prototype.getBlockByRef = function(blockRef){
  const self = this
  if (blockRef === 'latest') {
    return self._currentBlock
  } else {
    // if present, return block at reference
    let block = self._blockChain[blockRef]
    if (block) return block
    // check if we should create the new block
    const blockNum = Number(blockRef)
    if (blockNum > Number(self._currentBlock.number)) return
    // create, store, and return the new block
    block = createBlock({ number: blockRef })
    self._blockChain[blockRef] = block
    return block
  }
}

TestBlockProvider.prototype.nextBlock = function(blockParams){
  const self = this
  const newBlock = createBlock(blockParams, self._currentBlock, self._pendingTxs)
  self._pendingTxs = []
  self._currentBlock = newBlock
  self._blockChain[newBlock.number] = newBlock
  return self._currentBlock
}

TestBlockProvider.prototype.addTx = function(txParams){
  const self = this
  var newTx = extend({
    // defaults
    address: randomHash(),
    topics: [
      randomHash(),
      randomHash(),
      randomHash()
    ],
    data: randomHash(),
    blockNumber: '0xdeadbeef',
    logIndex: '0xdeadbeef',
    blockHash: '0x7c337eac9e3ec7bc99a1d911d326389558c9086afca7480a19698a16e40b2e0a',
    transactionHash: '0xd81da851bd3f4094d52cb86929e2ea3732a60ba7c184b853795fc5710a68b5fa',
    transactionIndex: '0x0'
    // provided
  }, txParams)
  self._pendingTxs.push(newTx)
  return newTx
}

function createBlock(blockParams, prevBlock, txs) {
  blockParams = blockParams || {}
  txs = txs || []
  var defaultNumber = prevBlock ? incrementHex(prevBlock.number) : '0x1'
  var defaultGasLimit = ethUtil.intToHex(4712388)
  return extend({
    // defaults
    number:            defaultNumber,
    hash:              randomHash(),
    parentHash:        prevBlock ? prevBlock.hash : randomHash(),
    nonce:             randomHash(),
    mixHash:           randomHash(),
    sha3Uncles:        randomHash(),
    logsBloom:         randomHash(),
    transactionsRoot:  randomHash(),
    stateRoot:         randomHash(),
    receiptsRoot:      randomHash(),
    miner:             randomHash(),
    difficulty:        randomHash(),
    totalDifficulty:   randomHash(),
    size:              randomHash(),
    extraData:         randomHash(),
    gasLimit:          defaultGasLimit,
    gasUsed:           randomHash(),
    timestamp:         randomHash(),
    transactions:      txs,
    // provided
  }, blockParams)
}

function incrementHex(hexString){
  return stripLeadingZeroes(ethUtil.intToHex(Number(hexString)+1))
}

function randomHash(){
  return ethUtil.bufferToHex(ethUtil.toBuffer(Math.floor(Math.random()*Number.MAX_SAFE_INTEGER)))
}

function stripLeadingZeroes (hexString) {
  let strippedHex = ethUtil.stripHexPrefix(hexString)
  while (strippedHex[0] === '0') {
    strippedHex = strippedHex.substr(1)
  }
  return ethUtil.addHexPrefix(strippedHex)
}
