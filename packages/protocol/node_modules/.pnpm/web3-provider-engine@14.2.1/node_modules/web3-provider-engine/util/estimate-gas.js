const createPayload = require('./create-payload.js')

module.exports = estimateGas

/*

This is a work around for https://github.com/ethereum/go-ethereum/issues/2577

*/


function estimateGas(provider, txParams, cb) {
  provider.sendAsync(createPayload({
    method: 'eth_estimateGas',
    params: [txParams]
  }), function(err, res){
    if (err) {
      // handle simple value transfer case
      if (err.message === 'no contract code at given address') {
        return cb(null, '0xcf08')
      } else {
        return cb(err)        
      }
    }
    cb(null, res.result)
  })
}