const waitForBlock = require('./waitForBlock')
const blockTagParamIndex = require('./cache-utils').blockTagParamIndex

module.exports = BlockRefRewriteMiddleware

function BlockRefRewriteMiddleware ({ blockTracker }) {
  if (!blockTracker) {
    throw Error('BlockRefRewriteMiddleware - mandatory "blockTracker" option is missing.')
  }

  return waitForBlock({ blockTracker })(handleRequest)

  // if blockRef is "latest", rewrite to latest block number
  function handleRequest (req, res, next, end) {
    const blockRefIndex = blockTagParamIndex(req)
    const blockRef = req.params[blockRefIndex]
    if (blockRef === 'latest') {
      let block = blockTracker.getCurrentBlock()
      req.params[blockRefIndex] = block.number
    }
    next()
  }
}
