const each = require('async/each')
const JsonRpcEngine = require('./index')
const asMiddleware = require('./asMiddleware')

module.exports = mergeMiddleware

function mergeMiddleware(middlewareStack) {
  const engine = new JsonRpcEngine()
  middlewareStack.forEach(middleware => engine.push(middleware))
  return asMiddleware(engine)
}
