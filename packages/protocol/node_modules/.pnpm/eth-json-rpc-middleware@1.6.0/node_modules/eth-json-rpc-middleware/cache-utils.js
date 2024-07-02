const stringify = require('json-stable-stringify')

module.exports = {
  cacheIdentifierForPayload: cacheIdentifierForPayload,
  canCache: canCache,
  blockTagForPayload: blockTagForPayload,
  paramsWithoutBlockTag: paramsWithoutBlockTag,
  blockTagParamIndex: blockTagParamIndex,
  cacheTypeForPayload: cacheTypeForPayload
}

function cacheIdentifierForPayload (payload, skipBlockRef) {
  const simpleParams = skipBlockRef ? paramsWithoutBlockTag(payload) : payload.params
  if (canCache(payload)) {
    return payload.method + ':' + stringify(simpleParams)
  } else {
    return null
  }
}

function canCache (payload) {
  return cacheTypeForPayload(payload) !== 'never'
}

function blockTagForPayload (payload) {
  let index = blockTagParamIndex(payload)

  // Block tag param not passed.
  if (index >= payload.params.length) {
    return null
  }

  return payload.params[index]
}

function paramsWithoutBlockTag (payload) {
  const index = blockTagParamIndex(payload)

  // Block tag param not passed.
  if (index >= payload.params.length) {
    return payload.params
  }
  
  // eth_getBlockByNumber has the block tag first, then the optional includeTx? param
  if (payload.method === 'eth_getBlockByNumber') {
    return payload.params.slice(1)
  }

  return payload.params.slice(0, index)
}

function blockTagParamIndex (payload) {
  switch (payload.method) {
    // blockTag is second param
    case 'eth_getBalance':
    case 'eth_getCode':
    case 'eth_getTransactionCount':
    case 'eth_getStorageAt':
    case 'eth_call':
    case 'eth_estimateGas':
      return 1
    // blockTag is first param
    case 'eth_getBlockByNumber':
      return 0
    // there is no blockTag
    default:
      return undefined
  }
}

function cacheTypeForPayload (payload) {
  switch (payload.method) {
    // cache permanently
    case 'web3_clientVersion':
    case 'web3_sha3':
    case 'eth_protocolVersion':
    case 'eth_getBlockTransactionCountByHash':
    case 'eth_getUncleCountByBlockHash':
    case 'eth_getCode':
    case 'eth_getBlockByHash':
    case 'eth_getTransactionByHash':
    case 'eth_getTransactionByBlockHashAndIndex':
    case 'eth_getTransactionReceipt':
    case 'eth_getUncleByBlockHashAndIndex':
    case 'eth_getCompilers':
    case 'eth_compileLLL':
    case 'eth_compileSolidity':
    case 'eth_compileSerpent':
    case 'shh_version':
    case 'test_permaCache':
      return 'perma'

    // cache until fork
    case 'eth_getBlockByNumber':
    case 'eth_getBlockTransactionCountByNumber':
    case 'eth_getUncleCountByBlockNumber':
    case 'eth_getTransactionByBlockNumberAndIndex':
    case 'eth_getUncleByBlockNumberAndIndex':
    case 'test_forkCache':
      return 'fork'

    // cache for block
    case 'eth_gasPrice':
    case 'eth_blockNumber':
    case 'eth_getBalance':
    case 'eth_getStorageAt':
    case 'eth_getTransactionCount':
    case 'eth_call':
    case 'eth_estimateGas':
    case 'eth_getFilterLogs':
    case 'eth_getLogs':
    case 'test_blockCache':
      return 'block'

    // never cache
    case 'net_version':
    case 'net_peerCount':
    case 'net_listening':
    case 'eth_syncing':
    case 'eth_sign':
    case 'eth_coinbase':
    case 'eth_mining':
    case 'eth_hashrate':
    case 'eth_accounts':
    case 'eth_sendTransaction':
    case 'eth_sendRawTransaction':
    case 'eth_newFilter':
    case 'eth_newBlockFilter':
    case 'eth_newPendingTransactionFilter':
    case 'eth_uninstallFilter':
    case 'eth_getFilterChanges':
    case 'eth_getWork':
    case 'eth_submitWork':
    case 'eth_submitHashrate':
    case 'db_putString':
    case 'db_getString':
    case 'db_putHex':
    case 'db_getHex':
    case 'shh_post':
    case 'shh_newIdentity':
    case 'shh_hasIdentity':
    case 'shh_newGroup':
    case 'shh_addToGroup':
    case 'shh_newFilter':
    case 'shh_uninstallFilter':
    case 'shh_getFilterChanges':
    case 'shh_getMessages':
    case 'test_neverCache':
      return 'never'
  }
}
