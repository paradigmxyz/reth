const traceTransaction = require('eth-tx-summary/trace-transaction')

module.exports = createTraceTransactionMiddleware

function createTraceTransactionMiddleware ({ provider }) {

  return (req, res, next, end) => {
    if (req.method !== 'debug_traceTransaction') return next()
    const [ targetTx ] = req.params
    traceTransaction(provider, targetTx, (err, result) => {
      if (err) return end(err)
      res.result = result
      end()
    })
  }

}
