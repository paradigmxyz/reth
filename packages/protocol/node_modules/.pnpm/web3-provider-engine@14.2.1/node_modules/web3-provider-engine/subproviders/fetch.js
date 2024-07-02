const fetch = require('cross-fetch')
const inherits = require('util').inherits
const retry = require('async/retry')
const waterfall = require('async/waterfall')
const asyncify = require('async/asyncify')
const JsonRpcError = require('json-rpc-error')
const promiseToCallback = require('promise-to-callback')
const createPayload = require('../util/create-payload.js')
const Subprovider = require('./subprovider.js')

const RETRIABLE_ERRORS = [
  // ignore server overload errors
  'Gateway timeout',
  'ETIMEDOUT',
  // ignore server sent html error pages
  // or truncated json responses
  'SyntaxError',
]

module.exports = RpcSource

inherits(RpcSource, Subprovider)

function RpcSource (opts) {
  const self = this
  self.rpcUrl = opts.rpcUrl
  self.originHttpHeaderKey = opts.originHttpHeaderKey
}

RpcSource.prototype.handleRequest = function (payload, next, end) {
  const self = this
  const originDomain = payload.origin

  // overwrite id to not conflict with other concurrent users
  const newPayload = createPayload(payload)
  // remove extra parameter from request
  delete newPayload.origin

  const reqParams = {
    method: 'POST',
    headers: {
      'Accept': 'application/json',
      'Content-Type': 'application/json'
    },
    body: JSON.stringify(newPayload)
  }

  if (self.originHttpHeaderKey && originDomain) {
    reqParams.headers[self.originHttpHeaderKey] = originDomain
  }

  retry({
    times: 5,
    interval: 1000,
    errorFilter: isErrorRetriable,
  },
  (cb) => self._submitRequest(reqParams, cb),
  (err, result) => {
    // ends on retriable error
    if (err && isErrorRetriable(err)) {
      const errMsg = `FetchSubprovider - cannot complete request. All retries exhausted.\nOriginal Error:\n${err.toString()}\n\n`
      const retriesExhaustedErr = new Error(errMsg)
      return end(retriesExhaustedErr)
    }
    // otherwise continue normally
    return end(err, result)
  })
}

RpcSource.prototype._submitRequest = function (reqParams, cb) {
  const self = this
  const targetUrl = self.rpcUrl

  promiseToCallback(fetch(targetUrl, reqParams))((err, res) => {
    if (err) return cb(err)

    // continue parsing result
    waterfall([
      checkForHttpErrors,
      // buffer body
      (cb) => promiseToCallback(res.text())(cb),
      // parse body
      asyncify((rawBody) => JSON.parse(rawBody)),
      parseResponse
    ], cb)

    function checkForHttpErrors (cb) {
      // check for errors
      switch (res.status) {
        case 405:
          return cb(new JsonRpcError.MethodNotFound())

        case 418:
          return cb(createRatelimitError())

        case 503:
        case 504:
          return cb(createTimeoutError())

        default:
          return cb()
      }
    }

    function parseResponse (body, cb) {
      // check for error code
      if (res.status !== 200) {
        return cb(new JsonRpcError.InternalError(body))
      }
      // check for rpc error
      if (body.error) return cb(new JsonRpcError.InternalError(body.error))
      // return successful result
      cb(null, body.result)
    }
  })
}

function isErrorRetriable(err){
  const errMsg = err.toString()
  return RETRIABLE_ERRORS.some(phrase => errMsg.includes(phrase))
}

function createRatelimitError () {
  let msg = `Request is being rate limited.`
  const err = new Error(msg)
  return new JsonRpcError.InternalError(err)
}

function createTimeoutError () {
  let msg = `Gateway timeout. The request took too long to process. `
  msg += `This can happen when querying logs over too wide a block range.`
  const err = new Error(msg)
  return new JsonRpcError.InternalError(err)
}
