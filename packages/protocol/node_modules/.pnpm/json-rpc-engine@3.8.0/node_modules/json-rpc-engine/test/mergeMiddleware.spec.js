/* eslint-env mocha */
'use strict'

const assert = require('assert')
const RpcEngine = require('../src/index.js')
const mergeMiddleware = require('../src/mergeMiddleware.js')

describe('mergeMiddleware', function () {
  it('basic', function (done) {
    let engine = new RpcEngine()
    let originalReq

    engine.push(mergeMiddleware([
      function (req, res, next, end) {
        originalReq = req
        res.result = 'saw merged middleware'
        end()
      }
    ]))

    let payload = { id: 1, jsonrpc: '2.0', method: 'hello' }

    engine.handle(payload, function (err, res) {
      assert.ifError(err, 'did not error')
      assert(res, 'has res')
      assert.equal(originalReq.id, res.id, 'id matches')
      assert.equal(originalReq.jsonrpc, res.jsonrpc, 'jsonrpc version matches')
      assert.equal(res.result, 'saw merged middleware', 'response was handled by nested middleware')
      done()
    })
  })

  it('handles next handler correctly for multiple merged', function (done) {
    let engine = new RpcEngine()

    engine.push(mergeMiddleware([
      (req, res, next, end) => {
        next((cb) => {
          res.copy = res.result
          cb()
        })
      },
      (req, res, next, end) => {
        res.result = true
        end()
      }
    ]))

    let payload = { id: 1, jsonrpc: '2.0', method: 'hello' }

    engine.handle(payload, function (err, res) {
      assert.ifError(err, 'did not error')
      assert(res, 'has res')
      assert.equal(res.result, res.copy, 'copied result correctly')
      done()
    })
  })

  it('decorate res', function (done) {
    let engine = new RpcEngine()
    let originalReq

    engine.push(mergeMiddleware([
      function (req, res, next, end) {
        originalReq = req
        res.xyz = true
        res.result = true
        end()
      }
    ]))

    let payload = { id: 1, jsonrpc: '2.0', method: 'hello' }

    engine.handle(payload, function (err, res) {
      assert.ifError(err, 'did not error')
      assert(res, 'has res')
      assert.equal(originalReq.id, res.id, 'id matches')
      assert.equal(originalReq.jsonrpc, res.jsonrpc, 'jsonrpc version matches')
      assert(res.xyz, 'res non-result prop was transfered')
      done()
    })
  })

  it('decorate req', function (done) {
    let engine = new RpcEngine()
    let originalReq

    engine.push(mergeMiddleware([
      function (req, res, next, end) {
        originalReq = req
        req.xyz = true
        res.result = true
        end()
      }
    ]))

    let payload = { id: 1, jsonrpc: '2.0', method: 'hello' }

    engine.handle(payload, function (err, res) {
      assert.ifError(err, 'did not error')
      assert(res, 'has res')
      assert.equal(originalReq.id, res.id, 'id matches')
      assert.equal(originalReq.jsonrpc, res.jsonrpc, 'jsonrpc version matches')
      assert(originalReq.xyz, 'req prop was transfered')
      done()
    })
  })

  it('should not error even if end not called', function (done) {
    let engine = new RpcEngine()

    engine.push(mergeMiddleware([
      (req, res, next, end) => next()
    ]))
    engine.push((req, res, next, end) => {
      res.result = true
      end()
    })

    let payload = { id: 1, jsonrpc: '2.0', method: 'hello' }

    engine.handle(payload, function (err, res) {
      assert.ifError(err, 'did not error')
      assert(res, 'has res')
      done()
    })
  })

  it('handles next handler correctly across middleware', function (done) {
    let engine = new RpcEngine()

    engine.push(mergeMiddleware([
      (req, res, next, end) => {
        next((cb) => {
          res.copy = res.result
          cb()
        })
      }
    ]))
    engine.push((req, res, next, end) => {
      res.result = true
      end()
    })
    let payload = { id: 1, jsonrpc: '2.0', method: 'hello' }

    engine.handle(payload, function (err, res) {
      assert.ifError(err, 'did not error')
      assert(res, 'has res')
      assert.equal(res.result, res.copy, 'copied result correctly')
      done()
    })
  })
})
