const ethUtil = require('ethereumjs-util')
const Tx = require('ethereumjs-tx')
const Trie = require('merkle-patricia-tree')
const BN = ethUtil.BN
const rlp = ethUtil.rlp
const async = require('async')
const BlockHeader = require('./header')
const params = require('ethereum-common/params.json')

/**
 * Creates a new block object
 * @constructor the raw serialized or the deserialized block.
 * @param {Array|Buffer|Object} data
 * @prop {Header} header the block's header
 * @prop {Array.<Header>} uncleList an array of uncle headers
 * @prop {Array.<Buffer>} raw an array of buffers containing the raw blocks.
 */
var Block = module.exports = function (data) {
  this.transactions = []
  this.uncleHeaders = []
  this._inBlockChain = false
  this.txTrie = new Trie()

  Object.defineProperty(this, 'raw', {
    get: function () {
      return this.serialize(false)
    }
  })

  var rawTransactions, rawUncleHeaders

  // defaults
  if (!data) {
    data = [[], [], []]
  }

  if (Buffer.isBuffer(data)) {
    data = rlp.decode(data)
  }

  if (Array.isArray(data)) {
    this.header = new BlockHeader(data[0])
    rawTransactions = data[1]
    rawUncleHeaders = data[2]
  } else {
    this.header = new BlockHeader(data.header)
    rawTransactions = data.transactions || []
    rawUncleHeaders = data.uncleHeaders || []
  }

  // parse uncle headers
  for (var i = 0; i < rawUncleHeaders.length; i++) {
    this.uncleHeaders.push(new BlockHeader(rawUncleHeaders[i]))
  }

  // parse transactions
  for (i = 0; i < rawTransactions.length; i++) {
    var tx = new Tx(rawTransactions[i])
    tx._homestead = true
    this.transactions.push(tx)
  }
}

Block.Header = BlockHeader

/**
 * Produces a hash the RLP of the block
 * @method hash
 */
Block.prototype.hash = function () {
  return this.header.hash()
}

/**
 * Determines if a given block is the genesis block
 * @method isGenisis
 * @return Boolean
 */
Block.prototype.isGenesis = function () {
  return this.header.isGenesis()
}

/**
 * turns the block in to the canonical genesis block
 * @method setGenesisParams
 */
Block.prototype.setGenesisParams = function () {
  this.header.gasLimit = params.genesisGasLimit.v
  this.header.difficulty = params.genesisDifficulty.v
  this.header.extraData = params.genesisExtraData.v
  this.header.nonce = params.genesisNonce.v
  this.header.stateRoot = params.genesisStateRoot.v
  this.header.number = new Buffer([])
}

/**
 * Produces a serialization of the block.
 * @method serialize
 * @param {Boolean} rlpEncode whether to rlp encode the block or not
 */
Block.prototype.serialize = function (rlpEncode) {
  var raw = [this.header.raw, [],
    []
  ]

  // rlpEnode defaults to true
  if (typeof rlpEncode === 'undefined') {
    rlpEncode = true
  }

  this.transactions.forEach(function (tx) {
    raw[1].push(tx.raw)
  })

  this.uncleHeaders.forEach(function (uncle) {
    raw[2].push(uncle.raw)
  })

  return rlpEncode ? rlp.encode(raw) : raw
}

/**
 * Generate transaction trie. The tx trie must be generated before the transaction trie can
 * be validated with `validateTransactionTrie`
 * @method genTxTrie
 * @param {Function} cb the callback
 */
Block.prototype.genTxTrie = function (cb) {
  var i = 0
  var self = this

  async.eachSeries(this.transactions, function (tx, done) {
    self.txTrie.put(rlp.encode(i), tx.serialize(), done)
    i++
  }, cb)
}

/**
 * Validates the transaction trie
 * @method validateTransactionTrie
 * @return {Boolean}
 */
Block.prototype.validateTransactionsTrie = function () {
  var txT = this.header.transactionsTrie.toString('hex')
  if (this.transactions.length) {
    return txT === this.txTrie.root.toString('hex')
  } else {
    return txT === ethUtil.SHA3_RLP.toString('hex')
  }
}

/**
 * Validates the transactions
 * @method validateTransactions
 * @param {Boolean} [stringError=false] whether to return a string with a dscription of why the validation failed or return a Bloolean
 * @return {Boolean}
 */
Block.prototype.validateTransactions = function (stringError) {
  var errors = []

  this.transactions.forEach(function (tx, i) {
    var error = tx.validate(true)
    if (error) {
      errors.push(error + ' at tx ' + i)
    }
  })

  if (stringError === undefined || stringError === false) {
    return errors.length === 0
  } else {
    return arrayToString(errors)
  }
}

/**
 * Validates the entire block. Returns a string to the callback if block is invalid
 * @method validate
 * @param {BlockChain} blockChain the blockchain that this block wants to be part of
 * @param {Function} cb the callback which is given a `String` if the block is not valid
 */
Block.prototype.validate = function (blockChain, cb) {
  var self = this
  var errors = []

  async.parallel([
    // validate uncles
    self.validateUncles.bind(self, blockChain),
    // validate block
    self.header.validate.bind(self.header, blockChain),
    // generate the transaction trie
    self.genTxTrie.bind(self)
  ], function (err) {
    if (err) {
      errors.push(err)
    }

    if (!self.validateTransactionsTrie()) {
      errors.push('invalid transaction true')
    }

    var txErrors = self.validateTransactions(true)
    if (txErrors !== '') {
      errors.push(txErrors)
    }

    if (!self.validateUnclesHash()) {
      errors.push('invild uncle hash')
    }

    cb(arrayToString(errors))
  })
}

/**
 * Validates the uncle's hash
 * @method validateUncleHash
 * @return {Boolean}
 */
Block.prototype.validateUnclesHash = function () {
  var raw = []
  this.uncleHeaders.forEach(function (uncle) {
    raw.push(uncle.raw)
  })

  raw = rlp.encode(raw)
  return ethUtil.sha3(raw).toString('hex') === this.header.uncleHash.toString('hex')
}

/**
 * Validates the uncles that are in the block if any. Returns a string to the callback if uncles are invalid
 * @method validateUncles
 * @param {Blockchain} blockChaina an instance of the Blockchain
 * @param {Function} cb the callback
 */
Block.prototype.validateUncles = function (blockChain, cb) {
  if (this.isGenesis()) {
    return cb()
  }

  var self = this

  if (self.uncleHeaders.length > 2) {
    return cb('too many uncle headers')
  }

  var uncleHashes = self.uncleHeaders.map(function (header) {
    return header.hash().toString('hex')
  })

  if (!((new Set(uncleHashes)).size === uncleHashes.length)) {
    return cb('dublicate unlces')
  }

  async.each(self.uncleHeaders, function (uncle, cb2) {
    var height = new BN(self.header.number)
    async.parallel([
      uncle.validate.bind(uncle, blockChain, height),
      // check to make sure the uncle is not already in the blockchain
      function (cb3) {
        blockChain.getDetails(uncle.hash(), function (err, blockInfo) {
          // TODO: remove uncles from BC
          if (blockInfo && blockInfo.isUncle) {
            cb3(err || 'uncle already included')
          } else {
            cb3()
          }
        })
      }
    ], cb2)
  }, cb)
}

/**
 * Converts the block toJSON
 * @method toJSON
 * @param {Bool} labeled whether to create an labeled object or an array
 * @return {Object}
 */
Block.prototype.toJSON = function (labeled) {
  if (labeled) {
    var obj = {
      header: this.header.toJSON(true),
      transactions: [],
      uncleHeaders: []
    }

    this.transactions.forEach(function (tx) {
      obj.transactions.push(tx.toJSON(labeled))
    })

    this.uncleHeaders.forEach(function (uh) {
      obj.uncleHeaders.push(uh.toJSON())
    })
    return obj
  } else {
    return ethUtil.baToJSON(this.raw)
  }
}

function arrayToString (array) {
  try {
    return array.reduce(function (str, err) {
      if (str) {
        str += ' '
      }
      return str + err
    })
  } catch (e) {
    return ''
  }
}
