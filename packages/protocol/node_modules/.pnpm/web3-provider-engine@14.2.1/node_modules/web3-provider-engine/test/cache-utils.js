const test = require('tape')
const cacheUtils = require('../util/rpc-cache-utils')

test('cacheIdentifierForPayload for latest block', function (t) {
  const payload1 = {id: 1, jsonrpc: '2.0', params: ['latest', false], method: 'eth_getBlockByNumber'}
  const payload2 = {id: 2, jsonrpc: '2.0', params: ['0x0', false], method: 'eth_getBlockByNumber'}
  const cacheId1 = cacheUtils.cacheIdentifierForPayload(payload1, { includeBlockRef: true })
  const cacheId2 = cacheUtils.cacheIdentifierForPayload(payload2, { includeBlockRef: true })

  t.notEqual(cacheId1, cacheId2, 'cacheIds are unique')
  t.end()
})

test('blockTagForPayload for different methods', function (t) {
  const payloads = [
    {jsonrpc: '2.0', method: 'eth_getBalance', params: ['0x407d73d8a49eeb85d32cf465507dd71d507100c1', '0x1234'], id: 1},
    {jsonrpc: '2.0', method: 'eth_getCode', params: ['0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b', '0x1234'], id: 1},
    {jsonrpc: '2.0', method: 'eth_getTransactionCount', params: ['0x407d73d8a49eeb85d32cf465507dd71d507100c1','0x1234'], id: 1},
    {jsonrpc: '2.0', method: 'eth_getStorageAt', params: ['0x295a70b2de5e3953354a6a8344e616ed314d7251', '0x0', '0x1234'], id: 1},
    {jsonrpc: '2.0', method: 'eth_call', params: [{to: '0x295a70b2de5e3953354a6a8344e616ed314d7251'}, '0x1234'], id: 1},
    {jsonrpc: '2.0', method: 'eth_estimateGas', params: [{to: '0x295a70b2de5e3953354a6a8344e616ed314d7251'}, '0x1234'], id: 1},
    {jsonrpc: '2.0', method: 'eth_getBlockByNumber', params: ['0x1234', true], id: 1},
  ]


  payloads.forEach(function (payload) {
    const blockTag = cacheUtils.blockTagForPayload(payload)
    t.isEqual(blockTag, '0x1234', 'block tag for ' + payload.method + ' is correct')
  })

  t.end()
})
