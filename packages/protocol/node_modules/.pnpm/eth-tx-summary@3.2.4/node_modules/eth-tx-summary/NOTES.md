// potential summary event schema

{
  type: 'call',
  params: {
    to: null,
    data: null,
    value: null,
  },
  summary: [],
  result: null,
  error: null,
}

{
  type: 'storageGet',
  key: null,
  value: null,
}

{
  type: 'storagePut',
  key: null,
  value: null,
}

function generateTxSummary(blockHash, txHash) {
  var blockchain = new BlockChain(blockDb)
  var trie = new StorageTrie(stateDb)
  blockchain.getBlock(blockHash, function(err, block){
    trie.root = block.stateRoot
  })
}