const async = require('async')
const clone = require('clone')
const EthQuery = require('eth-query')
const createRpcVm = require('ethereumjs-vm/dist/hooked').fromWeb3Provider
const ethUtil = require('ethereumjs-util')
// using local copy pending https://github.com/ethereumjs/ethereumjs-block/pull/24
const materializeBlock = require('./materialize-blocks')
// const materializeBlock = require('ethereumjs-block/from-rpc')
const Readable = require('stream').Readable
const ConcatStream = require('concat-stream')

module.exports = {
  createVmTraceStream,
  generateTxSummary,
}


function generateTxSummary(provider, txHash, cb) {
  const traceStream = createVmTraceStream(provider, txHash)
  traceStream.on('error', (err) => cb(err) )
  const concatStream = ConcatStream({ encoding: 'object' }, function(results){
    cb(null, results)
  })
  traceStream.pipe(concatStream)
}

function createVmTraceStream(provider, txHash){
  const traceStream = new Readable({ objectMode: true, read: noop })
  const query = new EthQuery(provider)

  // raw data
  let txData = null
  let blockData = null
  // eth objs
  let prepatoryTxs = null
  let targetTx = null
  let targetBlock = null
  let vm = null

  async.series({
    prepareVM,
    runPrepatoryTxs,
    runTargetTx,
  }, parseResults)

  return traceStream

  // load block data and create vm
  function prepareVM(cb){
    // load tx
    query.getTransactionByHash(txHash, function(err, _txData){
      if (err) return cb(err)
      if (!_txData) return cb(new Error('No transaction found...'))
      txData = _txData
      // load block
      // console.log('targetTx:',txData)
      traceStream.push({
        type: 'tx',
        data: txData,
      })
      query.getBlockByHash(txData.blockHash, true, function(err, _blockData){
        if (err) return cb(err)
        blockData = _blockData
        // materialize block and tx's
        targetBlock = materializeBlock(blockData)
        const txIndex = parseInt(txData.transactionIndex, 16)
        targetTx = targetBlock.transactions[txIndex]
        // determine prepatory tx's
        prepatoryTxs = targetBlock.transactions.slice(0, txIndex)
        // create vm
        // target tx's block's parent
        const backingStateBlockNumber = parseInt(blockData.number, 16) - 1
        const backingStateBlockNumberHex = `0x${backingStateBlockNumber.toString(16)}`
        vm = createRpcVm(provider, backingStateBlockNumberHex, {
          enableHomestead: true,
        })
        vm.on('error', console.error)
        // complete
        cb()
      })
    })
  }

  // we need to run all the txs to setup the state
  function runPrepatoryTxs(cb){
    async.eachSeries(prepatoryTxs, function(prepTx, cb){
      // console.log('prepTx!')
      vm.runTx({
        tx: prepTx,
        block: targetBlock,
        skipNonce: true,
        skipBalance: true,
      }, cb)
    }, cb)
  }

  // run the actual tx to analyze
  function runTargetTx(cb){
    const codePath = []
    let stepIndex = 0
    vm.on('step', function(step){
      const cleanStep = clone({
        index: stepIndex,
        pc: step.pc,
        gasLeft: step.gasLeft,
        opcode: step.opcode,
        stack: step.stack,
        depth: step.depth,
        address: step.address,
        account: step.account,
        cache: step.cache,
        memory: step.memory,
      })
      stepIndex++
      traceStream.push({
        type: 'step',
        data: cleanStep
      })
    })

    vm.runTx({
      tx: targetTx,
      block: targetBlock,
      skipNonce: true,
      skipBalance: true,
    }, function(err, results){
      if (err) return cb(err)
      cb(null, results)
    })
  }

  // return the summary
  function parseResults(err, data){
    if (err) throw err
    const results = data.runTargetTx
    traceStream.push({
      type: 'results',
      data: results,
    })
    traceStream.push(null)
  }

}

function noop(){}
