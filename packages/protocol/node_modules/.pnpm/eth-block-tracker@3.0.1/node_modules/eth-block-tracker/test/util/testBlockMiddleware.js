const createScaffoldMiddleware = require('json-rpc-engine/src/createScaffoldMiddleware')
const EthTx = require('ethereumjs-tx')
const ethUtil = require('ethereumjs-util')
const incrementHexNumber = require('../../src/hexUtils').incrementHexNumber
const formatHex = require('../../src/hexUtils').formatHex


module.exports = class TestBlockMiddleware {

  constructor() {
    this._blockchain = {}
    this._pendingTxs = []
    this.setCurrentBlock(createBlock({ number: '0x01' }))
  }

  nextBlock() {
    // prepare next block
    const nextNumber = incrementHexNumber(this.currentBlock.number)
    const nextBlock = createBlock({ number: nextNumber })
    // import pending txs into block
    nextBlock.transactions = this._pendingTxs
    this._pendingTxs = []
    // set as current block
    this.setCurrentBlock(nextBlock)
  }

  addTx (txParams) {
    const newTx = Object.assign({
      // defaults
      address: randomAddress(),
      topics: [
        randomHash(),
        randomHash(),
        randomHash()
      ],
      data: randomHash(),
      blockNumber: this.currentBlock.number,
      logIndex: '0xdeadbeef',
      blockHash: this.currentBlock.hash,
      hash: randomHash(),
      transactionIndex: '0x' + this._pendingTxs.length.toString(16)
      // provided
    }, txParams)
    this._pendingTxs.push(newTx)
    return newTx
  }

  setCurrentBlock(blockParams) {
    const newBlock = createBlock(blockParams)
    this.currentBlock = newBlock
    this._blockchain[newBlock.number] = newBlock
  }

  createMiddleware() {
    return createScaffoldMiddleware({

      eth_getBlockByNumber: (req, res, next, end) => {
        const blockRef = req.params[0]
        if (blockRef === 'latest') {
          res.result = this.currentBlock
        } else {
          res.result = this._blockchain[blockRef]
        }
        end()
      },

      eth_sendRawTransaction: (req, res, next, end) => {
        const rawTx = req.params[0]
        const rawTxBuffer = ethUtil.toBuffer(rawTx)
        const tx = new EthTx(rawTxBuffer)
        const txHash = ethUtil.bufferToHex(tx.hash())
        const txJson = {
          // raw data
          nonce: ethUtil.bufferToHex(tx.nonce),
          gasLimit: ethUtil.bufferToHex(tx.gasLimit),
          gasPrice: ethUtil.bufferToHex(tx.gasPrice),
          to: ethUtil.bufferToHex(tx.to),
          value: ethUtil.bufferToHex(tx.value),
          data: ethUtil.bufferToHex(tx.data),
          v: ethUtil.bufferToHex(tx.v),
          r: ethUtil.bufferToHex(tx.r),
          s: ethUtil.bufferToHex(tx.s),
          chainId: tx.chainId,
          // meta
          hash: txHash,
          address: ethUtil.bufferToHex(tx.from),
          topics: [],
          logIndex: '0xdeadbeef',
          transactionIndex: '0x' + this._pendingTxs.length.toString(16),
          blockNumber: this.currentBlock.number,
          blockHash: this.currentBlock.hash,
        }
        this.addTx(txJson)
        res.result = txHash
        end()
      },

    })
  }

}

function createBlock(blockParams){
  blockParams.number = formatHex(blockParams.number)
  return Object.assign({
    hash: randomHash(),
    difficulty: '0x2892ddca',
    extraData: '0x6574682d626c6f636b2d747261636b6572202d2066616b6520626c6f636b',
    gasLimit: '0x47e7c4',
    gasUsed: '0x6384',
    logsBloom: '0x00000000000000000000000000000000000000000000000000000000000020000000000000000000000000040000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000000000000000000',
    miner: randomAddress(),
    mixHash: randomHash(),
    nonce: randomHex(8),
    number: '0x72ac8',
    parentHash: randomHash(),
    receiptsRoot: randomHash(),
    sha3Uncles: randomHash(),
    stateRoot: randomHash(),
    timestamp: '0x58955844',
    totalDifficulty: '0x751d0dfa03c1',
    transactionsRoot: randomHash(),
    transactions: [],
    uncles: [],
  }, blockParams)
}

function randomHash(){
  return randomHex(32)
}

function randomAddress() {
  return randomHex(20)
}

function randomBuffer(len) {
  return Buffer.from(Array(len).fill().map(() => Math.floor(256*Math.random())))
}

function randomHex(len){
  return '0x' + randomBuffer(len).toString('hex')
}
