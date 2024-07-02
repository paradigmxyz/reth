const tape = require('tape')
const testing = require('ethereumjs-testing')
const rlp = require('ethereumjs-util').rlp
const Block = require('../index.js')

tape('[Block]: block functions', function (t) {
  const testData = require('./testdata.json')

  function testTransactionValidation (st, block) {
    st.equal(block.validateTransactions(), true)

    block.genTxTrie(function () {
      st.equal(block.validateTransactionsTrie(), true)
      st.end()
    })
  }

  t.test('should test transaction validation', function (st) {
    var block = new Block(rlp.decode(testData.blocks[0].rlp))
    st.plan(2)
    testTransactionValidation(st, block)
  })

  t.test('should test transaction validation with empty transaction list', function (st) {
    var block = new Block()
    st.plan(2)
    testTransactionValidation(st, block)
  })

  const testData2 = require('./testdata2.json')
  t.test('should test uncles hash validation', function (st) {
    var block = new Block(rlp.decode(testData2.blocks[2].rlp))
    st.equal(block.validateUnclesHash(), true)
    st.end()
  })

  t.test('should test isGenesis', function (st) {
    var block = new Block()
    st.notEqual(block.isGenesis(), true)
    block.header.number = new Buffer([])
    st.equal(block.isGenesis(), true)
    st.end()
  })

  const testDataGenesis = testing.getSingleFile('BasicTests/genesishashestest.json')
  t.test('should test genesis hashes', function (st) {
    var genesisBlock = new Block()
    genesisBlock.setGenesisParams()
    var rlp = genesisBlock.serialize()
    st.strictEqual(rlp.toString('hex'), testDataGenesis.genesis_rlp_hex, 'rlp hex match')
    st.strictEqual(genesisBlock.hash().toString('hex'), testDataGenesis.genesis_hash, 'genesis hash match')
    st.end()
  })

  t.test('should test toJSON', function (st) {
    var block = new Block(rlp.decode(testData2.blocks[2].rlp))
    st.equal(typeof (block.toJSON()), 'object')
    st.equal(typeof (block.toJSON(true)), 'object')
    st.end()
  })
})

