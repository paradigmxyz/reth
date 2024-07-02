const cacheIdentifierForPayload = require('./cache-utils').cacheIdentifierForPayload

module.exports = createInflightCache

function createInflightCache () {
  const inflightRequests = {}

  return (req, res, next, end) => {
    const cacheId = cacheIdentifierForPayload(req)
    // if not cacheable, skip
    if (!cacheId) return next()
    // check for matching requests
    let activeRequestHandlers = inflightRequests[cacheId]
    // if found, wait for the active request to be handled
    if (activeRequestHandlers) {
      // setup the response lister
      activeRequestHandlers.push((handledRes) => {
        // copy the result and error from the handledRes
        res.result = handledRes.result
        res.error = handledRes.error
        end()
      })
    } else {
      // setup response handler array for subsequent requests
      activeRequestHandlers = []
      inflightRequests[cacheId] = activeRequestHandlers
      // allow request to be handled normally
      next((done) => {
        // clear inflight requests
        delete inflightRequests[cacheId]
        // complete this request
        done()
        // once request has been handled, call all waiting handlers
        activeRequestHandlers.forEach((handler) => handler(res))
      })
    }
  }
}
