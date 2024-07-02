const blockTagForPayload = require('./cache-utils').blockTagForPayload

module.exports = waitForBlock

function waitForBlock ({ blockTracker }) {
  // validate arguments
  if (!blockTracker) {
    throw Error('WaitForBlockMiddleware - mandatory "blockTracker" option is missing.')
  }

  // return a factory that takes a middleware
  return (handleRequest) => {
    let currentHandler = null
    let requestQueue = []

    if (blockTracker.getCurrentBlock()) {
      // block tracker is already ready
      currentHandler = handleRequest
    } else {
      // buffer all requests for first block
      currentHandler = addToQueue
      // after first block
      blockTracker.once('latest', () => {
        // update handler
        currentHandler = handleRequest
        // process backlog
        requestQueue.forEach((args) => handleRequest.apply(null, args))
        requestQueue = null
      })
    }

    // add requst to queue if blockRef is "latest"
    function addToQueue (req, res, next, end) {
      const blockRef = blockTagForPayload(req)
      if (blockRef === 'latest') {
        requestQueue.push([req, res, next, end])
      } else {
        next()
      }
    }

    return (req, res, next, end) => {
      currentHandler(req, res, next, end)
    }
  }
}
