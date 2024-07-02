const fetch = global.fetch || require('fetch-ponyfill')().fetch
const retry = require('async/retry')
const waterfall = require('async/waterfall')
const asyncify = require('async/asyncify')
const JsonRpcError = require('json-rpc-error')
const promiseToCallback = require('promise-to-callback')

module.exports = createFetchMiddleware

function createFetchMiddleware ({ rpcUrl, originHttpHeaderKey }) {
  return (req, res, next, end) => {
    const payload = Object.assign({}, req)

    // extract 'origin' parameter from request
    const originDomain = payload.origin
    delete payload.origin

    const httpReqParams = {
      method: 'POST',
      headers: {
        'Accept': 'application/json',
        'Content-Type': 'application/json'
      },
      body: JSON.stringify(payload)
    }

    if (originHttpHeaderKey && originDomain) {
      httpReqParams.headers[originHttpHeaderKey] = originDomain
    }

    retry({
      times: 5,
      interval: 1000,
      errorFilter: (err) => {
        // ignore server overload errors
        err.message.includes('Gateway timeout')
        // ignore server sent html error pages
        // or truncated json responses
        || err.message.includes('JSON')
      },
    }, (cb) => {
      let fetchRes
      let fetchBody
      waterfall([
        // make request
        (cb) => promiseToCallback(fetch(rpcUrl, httpReqParams))(cb),
        asyncify((_fetchRes) => { fetchRes = _fetchRes }),
        // check for http errrors
        (_, cb) => checkForHttpErrors(fetchRes, cb),
        // buffer body
        (cb) => promiseToCallback(fetchRes.text())(cb),
        asyncify((rawBody) => { fetchBody = JSON.parse(rawBody) }),
        // parse response body
        (_, cb) => parseResponse(fetchRes, fetchBody, cb)
      ], cb)
    }, (err, result) => {
      if (err) return end(err)
      // append result and complete
      res.result = result
      end()
    })
  }
}

function checkForHttpErrors (res, cb) {
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

function parseResponse (res, body, cb) {
  // check for error code
  if (res.status !== 200) {
    return cb(new JsonRpcError.InternalError(body))
  }
  // check for rpc error
  if (body.error) return cb(new JsonRpcError.InternalError(body.error))
  // return successful result
  cb(null, body.result)
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
