const async = require('async')
const inherits = require('util').inherits
const ethUtil = require('ethereumjs-util')
const Subprovider = require('./subprovider.js')
const Stoplight = require('../util/stoplight.js')
const EventEmitter = require('events').EventEmitter

module.exports = FilterSubprovider

// handles the following RPC methods:
//   eth_newBlockFilter
//   eth_newPendingTransactionFilter
//   eth_newFilter
//   eth_getFilterChanges
//   eth_uninstallFilter
//   eth_getFilterLogs

inherits(FilterSubprovider, Subprovider)

function FilterSubprovider(opts) {
  opts = opts || {}
  const self = this
  self.filterIndex = 0
  self.filters = {}
  self.filterDestroyHandlers = {}
  self.asyncBlockHandlers = {}
  self.asyncPendingBlockHandlers = {}
  self._ready = new Stoplight()
  self._ready.setMaxListeners(opts.maxFilters || 25)
  self._ready.go()
  self.pendingBlockTimeout = opts.pendingBlockTimeout || 4000
  self.checkForPendingBlocksActive = false

  // we dont have engine immeditately
  setTimeout(function(){
    // asyncBlockHandlers require locking provider until updates are completed
    self.engine.on('block', function(block){
      // pause processing
      self._ready.stop()
      // update filters
      var updaters = valuesFor(self.asyncBlockHandlers)
      .map(function(fn){ return fn.bind(null, block) })
      async.parallel(updaters, function(err){
        if (err) console.error(err)
        // unpause processing
        self._ready.go()
      })
    })
  })

}

FilterSubprovider.prototype.handleRequest = function(payload, next, end){
  const self = this
  switch(payload.method){

    case 'eth_newBlockFilter':
      self.newBlockFilter(end)
      return

    case 'eth_newPendingTransactionFilter':
      self.newPendingTransactionFilter(end)
      self.checkForPendingBlocks()
      return

    case 'eth_newFilter':
      self.newLogFilter(payload.params[0], end)
      return

    case 'eth_getFilterChanges':
      self._ready.await(function(){
        self.getFilterChanges(payload.params[0], end)
      })
      return

    case 'eth_getFilterLogs':
      self._ready.await(function(){
        self.getFilterLogs(payload.params[0], end)
      })
      return

    case 'eth_uninstallFilter':
      self._ready.await(function(){
        self.uninstallFilter(payload.params[0], end)
      })
      return

    default:
      next()
      return
  }
}

FilterSubprovider.prototype.newBlockFilter = function(cb) {
  const self = this

  self._getBlockNumber(function(err, blockNumber){
    if (err) return cb(err)

    var filter = new BlockFilter({
      blockNumber: blockNumber,
    })

    var newBlockHandler = filter.update.bind(filter)
    self.engine.on('block', newBlockHandler)
    var destroyHandler = function(){
      self.engine.removeListener('block', newBlockHandler)
    }

    self.filterIndex++
    self.filters[self.filterIndex] = filter
    self.filterDestroyHandlers[self.filterIndex] = destroyHandler

    var hexFilterIndex = intToHex(self.filterIndex)
    cb(null, hexFilterIndex)
  })
}

FilterSubprovider.prototype.newLogFilter = function(opts, cb) {
  const self = this

  self._getBlockNumber(function(err, blockNumber){
    if (err) return cb(err)

    var filter = new LogFilter(opts)
    var newLogHandler = filter.update.bind(filter)
    var blockHandler = function(block, cb){
      self._logsForBlock(block, function(err, logs){
        if (err) return cb(err)
        newLogHandler(logs)
        cb()
      })
    }

    self.filterIndex++
    self.asyncBlockHandlers[self.filterIndex] = blockHandler
    self.filters[self.filterIndex] = filter

    var hexFilterIndex = intToHex(self.filterIndex)
    cb(null, hexFilterIndex)
  })
}

FilterSubprovider.prototype.newPendingTransactionFilter = function(cb) {
  const self = this

  var filter = new PendingTransactionFilter()
  var newTxHandler = filter.update.bind(filter)
  var blockHandler = function(block, cb){
    self._txHashesForBlock(block, function(err, txs){
      if (err) return cb(err)
      newTxHandler(txs)
      cb()
    })
  }

  self.filterIndex++
  self.asyncPendingBlockHandlers[self.filterIndex] = blockHandler
  self.filters[self.filterIndex] = filter

  var hexFilterIndex = intToHex(self.filterIndex)
  cb(null, hexFilterIndex)
}

FilterSubprovider.prototype.getFilterChanges = function(hexFilterId, cb) {
  const self = this

  var filterId = Number.parseInt(hexFilterId, 16)
  var filter = self.filters[filterId]
  if (!filter) console.warn('FilterSubprovider - no filter with that id:', hexFilterId)
  if (!filter) return cb(null, [])
  var results = filter.getChanges()
  filter.clearChanges()
  cb(null, results)
}

FilterSubprovider.prototype.getFilterLogs = function(hexFilterId, cb) {
  const self = this

  var filterId = Number.parseInt(hexFilterId, 16)
  var filter = self.filters[filterId]
  if (!filter) console.warn('FilterSubprovider - no filter with that id:', hexFilterId)
  if (!filter) return cb(null, [])
  if (filter.type === 'log') {
    self.emitPayload({
      method: 'eth_getLogs',
      params: [{
        fromBlock: filter.fromBlock,
        toBlock: filter.toBlock,
        address: filter.address,
        topics: filter.topics,
      }],
    }, function(err, res){
      if (err) return cb(err)
      cb(null, res.result)
    })
  } else {
    var results = []
    cb(null, results)
  }
}

FilterSubprovider.prototype.uninstallFilter = function(hexFilterId, cb) {
  const self = this

  var filterId = Number.parseInt(hexFilterId, 16)
  var filter = self.filters[filterId]
  if (!filter) {
    cb(null, false)
    return
  }

  self.filters[filterId].removeAllListeners()

  var destroyHandler = self.filterDestroyHandlers[filterId]
  delete self.filters[filterId]
  delete self.asyncBlockHandlers[filterId]
  delete self.asyncPendingBlockHandlers[filterId]
  delete self.filterDestroyHandlers[filterId]
  if (destroyHandler) destroyHandler()

  cb(null, true)
}

// private

// check for pending blocks
FilterSubprovider.prototype.checkForPendingBlocks = function(){
  const self = this
  if (self.checkForPendingBlocksActive) return
  var activePendingTxFilters = !!Object.keys(self.asyncPendingBlockHandlers).length
  if (activePendingTxFilters) {
    self.checkForPendingBlocksActive = true
    self.emitPayload({
      method: 'eth_getBlockByNumber',
      params: ['pending', true],
    }, function(err, res){
      if (err) {
        self.checkForPendingBlocksActive = false
        console.error(err)
        return
      }
      self.onNewPendingBlock(res.result, function(err){
        if (err) console.error(err)
        self.checkForPendingBlocksActive = false
        setTimeout(self.checkForPendingBlocks.bind(self), self.pendingBlockTimeout)
      })
    })
  }
}

FilterSubprovider.prototype.onNewPendingBlock = function(block, cb){
  const self = this
  // update filters
  var updaters = valuesFor(self.asyncPendingBlockHandlers)
  .map(function(fn){ return fn.bind(null, block) })
  async.parallel(updaters, cb)
}

FilterSubprovider.prototype._getBlockNumber = function(cb) {
  const self = this
  var blockNumber = bufferToNumberHex(self.engine.currentBlock.number)
  cb(null, blockNumber)
}

FilterSubprovider.prototype._logsForBlock = function(block, cb) {
  const self = this
  var blockNumber = bufferToNumberHex(block.number)
  self.emitPayload({
    method: 'eth_getLogs',
    params: [{
      fromBlock: blockNumber,
      toBlock: blockNumber,
    }],
  }, function(err, response){
    if (err) return cb(err)
    if (response.error) return cb(response.error)
    cb(null, response.result)
  })

}

FilterSubprovider.prototype._txHashesForBlock = function(block, cb) {
  const self = this
  var txs = block.transactions
  // short circuit if empty
  if (txs.length === 0) return cb(null, [])
  // txs are already hashes
  if ('string' === typeof txs[0]) {
    cb(null, txs)
  // txs are obj, need to map to hashes
  } else {
    var results = txs.map((tx) => tx.hash)
    cb(null, results)
  }
}

//
// BlockFilter
//

inherits(BlockFilter, EventEmitter)

function BlockFilter(opts) {
  // console.log('BlockFilter - new')
  const self = this
  EventEmitter.apply(self)
  self.type = 'block'
  self.engine = opts.engine
  self.blockNumber = opts.blockNumber
  self.updates = []
}

BlockFilter.prototype.update = function(block){
  // console.log('BlockFilter - update')
  const self = this
  var blockHash = bufferToHex(block.hash)
  self.updates.push(blockHash)
  self.emit('data', block)
}

BlockFilter.prototype.getChanges = function(){
  const self = this
  var results = self.updates
  // console.log('BlockFilter - getChanges:', results.length)
  return results
}

BlockFilter.prototype.clearChanges = function(){
  // console.log('BlockFilter - clearChanges')
  const self = this
  self.updates = []
}

//
// LogFilter
//

inherits(LogFilter, EventEmitter)

function LogFilter(opts) {
  // console.log('LogFilter - new')
  const self = this
  EventEmitter.apply(self)
  self.type = 'log'
  self.fromBlock = (opts.fromBlock !== undefined) ? opts.fromBlock : 'latest'
  self.toBlock = (opts.toBlock !== undefined) ? opts.toBlock : 'latest'
  var expectedAddress = opts.address && (Array.isArray(opts.address) ? opts.address : [opts.address]);
  self.address = expectedAddress && expectedAddress.map(normalizeHex);
  self.topics = opts.topics || []
  self.updates = []
  self.allResults = []
}

LogFilter.prototype.validateLog = function(log){
  // console.log('LogFilter - validateLog:', log)
  const self = this

  // check if block number in bounds:
  // console.log('LogFilter - validateLog - blockNumber', self.fromBlock, self.toBlock)
  if (blockTagIsNumber(self.fromBlock) && hexToInt(self.fromBlock) >= hexToInt(log.blockNumber)) return false
  if (blockTagIsNumber(self.toBlock) && hexToInt(self.toBlock) <= hexToInt(log.blockNumber)) return false

  // address is correct:
  // console.log('LogFilter - validateLog - address', self.address)
  if (self.address && !(self.address.map((a) => a.toLowerCase()).includes(
    log.address.toLowerCase()))) return false

  // topics match:
  // topics are position-dependant
  // topics can be nested to represent `or` [[a || b], c]
  // topics can be null, representing a wild card for that position
  // console.log('LogFilter - validateLog - topics', log.topics)
  // console.log('LogFilter - validateLog - against topics', self.topics)
  var topicsMatch = self.topics.reduce(function(previousMatched, topicPattern, index){
    // abort in progress
    if (!previousMatched) return false
    // wild card
    if (!topicPattern) return true
    // pattern is longer than actual topics
    var logTopic = log.topics[index]
    if (!logTopic) return false
    // check each possible matching topic
    var subtopicsToMatch = Array.isArray(topicPattern) ? topicPattern : [topicPattern]
    var topicDoesMatch = subtopicsToMatch.filter(function(subTopic) {
      return logTopic.toLowerCase() === subTopic.toLowerCase()
    }).length > 0
    return topicDoesMatch
  }, true)

  // console.log('LogFilter - validateLog - '+(topicsMatch ? 'approved!' : 'denied!')+' ==============')
  return topicsMatch
}

LogFilter.prototype.update = function(logs){
  // console.log('LogFilter - update')
  const self = this
  // validate filter match
  var validLogs = []
  logs.forEach(function(log) {
    var validated = self.validateLog(log)
    if (!validated) return
    // add to results
    validLogs.push(log)
    self.updates.push(log)
    self.allResults.push(log)
  })
  if (validLogs.length > 0) {
    self.emit('data', validLogs)
  }
}

LogFilter.prototype.getChanges = function(){
  // console.log('LogFilter - getChanges')
  const self = this
  var results = self.updates
  return results
}

LogFilter.prototype.getAllResults = function(){
  // console.log('LogFilter - getAllResults')
  const self = this
  var results = self.allResults
  return results
}

LogFilter.prototype.clearChanges = function(){
  // console.log('LogFilter - clearChanges')
  const self = this
  self.updates = []
}

//
// PendingTxFilter
//

inherits(PendingTransactionFilter, EventEmitter)

function PendingTransactionFilter(){
  // console.log('PendingTransactionFilter - new')
  const self = this
  EventEmitter.apply(self)
  self.type = 'pendingTx'
  self.updates = []
  self.allResults = []
}

PendingTransactionFilter.prototype.validateUnique = function(tx){
  const self = this
  return self.allResults.indexOf(tx) === -1
}

PendingTransactionFilter.prototype.update = function(txs){
  // console.log('PendingTransactionFilter - update')
  const self = this
  var validTxs = []
  txs.forEach(function (tx) {
    // validate filter match
    var validated = self.validateUnique(tx)
    if (!validated) return
    // add to results
    validTxs.push(tx)
    self.updates.push(tx)
    self.allResults.push(tx)
  })
  if (validTxs.length > 0) {
    self.emit('data', validTxs)
  }
}

PendingTransactionFilter.prototype.getChanges = function(){
  // console.log('PendingTransactionFilter - getChanges')
  const self = this
  var results = self.updates
  return results
}

PendingTransactionFilter.prototype.getAllResults = function(){
  // console.log('PendingTransactionFilter - getAllResults')
  const self = this
  var results = self.allResults
  return results
}

PendingTransactionFilter.prototype.clearChanges = function(){
  // console.log('PendingTransactionFilter - clearChanges')
  const self = this
  self.updates = []
}

// util

function normalizeHex(hexString) {
  return hexString.slice(0, 2) === '0x' ? hexString : '0x'+hexString
}

function intToHex(value) {
  return ethUtil.intToHex(value)
}

function hexToInt(hexString) {
  return Number(hexString)
}

function bufferToHex(buffer) {
  return '0x'+buffer.toString('hex')
}

function bufferToNumberHex(buffer) {
  return stripLeadingZero(buffer.toString('hex'))
}

function stripLeadingZero(hexNum) {
  let stripped = ethUtil.stripHexPrefix(hexNum)
  while (stripped[0] === '0') {
    stripped = stripped.substr(1)
  }
  return `0x${stripped}`
}

function blockTagIsNumber(blockTag){
  return blockTag && ['earliest', 'latest', 'pending'].indexOf(blockTag) === -1
}

function valuesFor(obj){
  return Object.keys(obj).map(function(key){ return obj[key] })
}
