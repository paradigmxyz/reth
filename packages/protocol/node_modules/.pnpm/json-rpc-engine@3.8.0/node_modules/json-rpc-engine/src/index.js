'use strict'
const async = require('async')
const SafeEventEmitter = require('safe-event-emitter')

class RpcEngine extends SafeEventEmitter {
  constructor () {
    super()
    this._middleware = []
  }

  //
  // Public
  //

  push (middleware) {
    this._middleware.push(middleware)
  }

  handle (req, cb) {
    // batch request support
    if (Array.isArray(req)) {
      async.map(req, this._handle.bind(this), cb)
    } else {
      this._handle(req, cb)
    }
  }

  //
  // Private
  //

  _handle (_req, cb) {
    // shallow clone request object
    const req = Object.assign({}, _req)
    // create response obj
    const res = {
      id: req.id,
      jsonrpc: req.jsonrpc
    }
    // process all middleware
    this._runMiddleware(req, res, (err) => {
      // return response
      cb(err, res)
    })
  }

  _runMiddleware (req, res, onDone) {
    // flow
    async.waterfall([
      (cb) => this._runMiddlewareDown(req, res, cb),
      checkForCompletion,
      (returnHandlers, cb) => this._runReturnHandlersUp(returnHandlers, cb),
    ], onDone)

    function checkForCompletion({ isComplete, returnHandlers }, cb) {
      // fail if not completed
      if (!('result' in res) && !('error' in res)) {
        const requestBody = JSON.stringify(req, null, 2)
        const message = 'JsonRpcEngine - response has no error or result for request:\n' + requestBody
        return cb(new Error(message))
      }
      if (!isComplete) {
        const requestBody = JSON.stringify(req, null, 2)
        const message = 'JsonRpcEngine - nothing ended request:\n' + requestBody
        return cb(new Error(message))
      }
      // continue
      return cb(null, returnHandlers)
    }

    function runReturnHandlers (returnHandlers, cb) {
      async.eachSeries(returnHandlers, (handler, next) => handler(next), onDone)
    }
  }

  // walks down stack of middleware
  _runMiddlewareDown (req, res, onDone) {
    // for climbing back up the stack
    let allReturnHandlers = []
    // flag for stack return
    let isComplete = false

    // down stack of middleware, call and collect optional allReturnHandlers
    async.mapSeries(this._middleware, eachMiddleware, completeRequest)

    // runs an individual middleware
    function eachMiddleware (middleware, cb) {
      // skip middleware if completed
      if (isComplete) return cb()
      // run individual middleware
      middleware(req, res, next, end)

      function next (returnHandler) {
        // add return handler
        allReturnHandlers.push(returnHandler)
        cb()
      }
      function end (err) {
        if (err) return cb(err)
        // mark as completed
        isComplete = true
        cb()
      }
    }

    // returns, indicating whether or not it ended
    function completeRequest (err) {
      if (err) {
        // prepare error message
        res.error = {
          code: err.code || -32603,
          message: err.stack,
        }
        // return error-first and res with err
        return onDone(err, res)
      }
      const returnHandlers = allReturnHandlers.filter(Boolean).reverse()
      onDone(null, { isComplete, returnHandlers })
    }
  }

  // climbs the stack calling return handlers
  _runReturnHandlersUp (returnHandlers, cb) {
    async.eachSeries(returnHandlers, (handler, next) => handler(next), cb)
  }
}

module.exports = RpcEngine
