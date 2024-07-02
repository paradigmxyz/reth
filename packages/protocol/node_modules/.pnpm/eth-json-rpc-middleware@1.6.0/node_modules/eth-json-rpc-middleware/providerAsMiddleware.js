module.exports = providerAsMiddleware

function providerAsMiddleware (provider) {
  return (req, res, next, end) => {
    // send request to provider
    provider.sendAsync(req, (err, providerRes) => {
      // forward any error
      if (err) return end(err)
      // copy provider response onto original response
      Object.assign(res, providerRes)
      end()
    })
  }
}
