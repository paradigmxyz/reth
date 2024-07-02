'use strict'
const tape = require('tape')
const blockFromRpc = require('../from-rpc.js')
const blockData = require('./testdata-from-rpc.json')

tape('[fromRPC]: block #2924874', function (t) {
  t.test('should create a block with transactions with valid signatures', function (st) {
    let block = blockFromRpc(blockData)
    let allValid = block.transactions.every(tx => tx.verifySignature())
    st.equal(allValid, true, 'all transaction signatures are valid')
    st.end()
  })
})
