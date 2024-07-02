const utils = require('ethereumjs-util')
const params = require('ethereum-common/params.json')
const BN = utils.BN
  /**
   * An object that repersents the block header
   * @constructor
   * @param {Array} data raw data, deserialized
   * @prop {Buffer} parentHash the blocks' parent's hash
   * @prop {Buffer} uncleHash sha3(rlp_encode(uncle_list))
   * @prop {Buffer} coinbase the miner address
   * @prop {Buffer} stateRoot The root of a Merkle Patricia tree
   * @prop {Buffer} transactionTrie the root of a Trie containing the transactions
   * @prop {Buffer} receiptTrie the root of a Trie containing the transaction Reciept
   * @prop {Buffer} bloom
   * @prop {Buffer} difficulty
   * @prop {Buffer} number the block's height
   * @prop {Buffer} gasLimit
   * @prop {Buffer} gasUsed
   * @prop {Buffer} timestamp
   * @prop {Buffer} extraData
   * @prop {Array.<Buffer>} raw an array of buffers containing the raw blocks.
   */
var BlockHeader = module.exports = function (data) {
  var fields = [{
    name: 'parentHash',
    length: 32,
    default: utils.zeros(32)
  }, {
    name: 'uncleHash',
    default: utils.SHA3_RLP_ARRAY
  }, {
    name: 'coinbase',
    length: 20,
    default: utils.zeros(20)
  }, {
    name: 'stateRoot',
    length: 32,
    default: utils.zeros(32)
  }, {
    name: 'transactionsTrie',
    length: 32,
    default: utils.SHA3_RLP
  }, {
    name: 'receiptTrie',
    length: 32,
    default: utils.SHA3_RLP
  }, {
    name: 'bloom',
    default: utils.zeros(256)
  }, {
    name: 'difficulty',
    default: new Buffer([])
  }, {
    name: 'number',
    default: utils.intToBuffer(params.homeSteadForkNumber.v)
  }, {
    name: 'gasLimit',
    default: new Buffer('ffffffffffffff', 'hex')
  }, {
    name: 'gasUsed',
    empty: true,
    default: new Buffer([])
  }, {
    name: 'timestamp',
    default: new Buffer([])
  }, {
    name: 'extraData',
    allowZero: true,
    empty: true,
    default: new Buffer([])
  }, {
    name: 'mixHash',
    default: utils.zeros(32)
      // length: 32
  }, {
    name: 'nonce',
    default: new Buffer([]) // sha3(42)
  }]
  utils.defineProperties(this, fields, data)
}

/**
 * Returns the canoncical difficulty of the block
 * @method canonicalDifficulty
 * @param {Block} parentBlock the parent `Block` of the this header
 * @return {BN}
 */
BlockHeader.prototype.canonicalDifficulty = function (parentBlock) {
  const blockTs = new BN(this.timestamp)
  const parentTs = new BN(parentBlock.header.timestamp)
  const parentDif = new BN(parentBlock.header.difficulty)
  const minimumDifficulty = new BN(params.minimumDifficulty.v)
  var offset = parentDif.div(new BN(params.difficultyBoundDivisor.v))
  var dif

  // Byzantium
  // max((2 if len(parent.uncles) else 1) - ((timestamp - parent.timestamp) // 9), -99)
  var uncleAddend = parentBlock.header.uncleHash.equals(utils.SHA3_RLP_ARRAY) ? 1 : 2
  var a = blockTs.sub(parentTs).idivn(9).ineg().iaddn(uncleAddend)
  var cutoff = new BN(-99)
  // MAX(cutoff, a)
  if (cutoff.cmp(a) === 1) {
    a = cutoff
  }
  dif = parentDif.add(offset.mul(a))

  // Byzantium difficulty bomb delay
  var num = new BN(this.number).isubn(3000000)
  if (num.ltn(0)) {
    num = new BN(0)
  }

  var exp = num.idivn(100000).isubn(2)
  if (!exp.isNeg()) {
    dif.iadd(new BN(2).pow(exp))
  }

  if (dif.cmp(minimumDifficulty) === -1) {
    dif = minimumDifficulty
  }

  return dif
}

/**
 * checks that the block's `difficuly` matches the canonical difficulty
 * @method validateDifficulty
 * @param {Block} parentBlock this block's parent
 * @return {Boolean}
 */
BlockHeader.prototype.validateDifficulty = function (parentBlock) {
  const dif = this.canonicalDifficulty(parentBlock)
  return dif.cmp(new BN(this.difficulty)) === 0
}

/**
 * Validates the gasLimit
 * @method validateGasLimit
 * @param {Block} parentBlock this block's parent
 * @returns {Boolean}
 */
BlockHeader.prototype.validateGasLimit = function (parentBlock) {
  const pGasLimit = new BN(parentBlock.header.gasLimit)
  const gasLimit = new BN(this.gasLimit)
  const a = pGasLimit.div(new BN(params.gasLimitBoundDivisor.v))
  const maxGasLimit = pGasLimit.add(a)
  const minGasLimit = pGasLimit.sub(a)

  return gasLimit.lt(maxGasLimit) && gasLimit.gt(minGasLimit) && gasLimit.gte(params.minGasLimit.v)
}

/**
 * Validates the entire block header
 * @method validate
 * @param {Blockchain} blockChain the blockchain that this block is validating against
 * @param {Bignum} [height] if this is an uncle header, this is the height of the block that is including it
 * @param {Function} cb the callback function. The callback is given an `error` if the block is invalid
 */
BlockHeader.prototype.validate = function (blockchain, height, cb) {
  var self = this
  if (arguments.length === 2) {
    cb = height
    height = false
  }

  if (this.isGenesis()) {
    return cb()
  }

  // find the blocks parent
  blockchain.getBlock(self.parentHash, function (err, parentBlock) {
    if (err) {
      return cb('could not find parent block')
    }

    self.parentBlock = parentBlock

    var number = new BN(self.number)
    if (number.cmp(new BN(parentBlock.header.number).iaddn(1)) !== 0) {
      return cb('invalid number')
    }

    if (height) {
      var dif = height.sub(new BN(parentBlock.header.number))
      if (!(dif.cmpn(8) === -1 && dif.cmpn(1) === 1)) {
        return cb('uncle block has a parent that is too old or to young')
      }
    }

    if (!self.validateDifficulty(parentBlock)) {
      return cb('invalid Difficulty')
    }

    if (!self.validateGasLimit(parentBlock)) {
      return cb('invalid gas limit')
    }

    if (utils.bufferToInt(parentBlock.header.number) + 1 !== utils.bufferToInt(self.number)) {
      return cb('invalid heigth')
    }

    if (utils.bufferToInt(self.timestamp) <= utils.bufferToInt(parentBlock.header.timestamp)) {
      return cb('invalid timestamp')
    }

    if (self.extraData.length > params.maximumExtraDataSize.v) {
      return cb('invalid amount of extra data')
    }

    cb()
  })
}

/**
 * Returns the sha3 hash of the blockheader
 * @method hash
 * @return {Buffer}
 */
BlockHeader.prototype.hash = function () {
  return utils.rlphash(this.raw)
}

/**
 * checks if the blockheader is a genesis header
 * @method isGenesis
 * @return {Boolean}
 */
BlockHeader.prototype.isGenesis = function () {
  return this.number.toString('hex') === ''
}

