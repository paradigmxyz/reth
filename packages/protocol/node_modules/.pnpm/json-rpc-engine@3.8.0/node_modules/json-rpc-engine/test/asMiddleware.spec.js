/* eslint-env mocha */
'use strict'

const assert = require('assert')
const RpcEngine = require('../src/index.js')
const asMiddleware = require('../src/asMiddleware.js')

describe('asMiddleware', function () {
  it('basic', function (done) {
    let engine = new RpcEngine()
    let subengine = new RpcEngine()
    let originalReq

    subengine.push(function (req, res, next, end) {
      originalReq = req
      res.result = 'saw subengine'
      end()
    })

    engine.push(asMiddleware(subengine))

    let payload = { id: 1, jsonrpc: '2.0', method: 'hello' }

    engine.handle(payload, function (err, res) {
      assert.ifError(err, 'did not error')
      assert(res, 'has res')
      assert.equal(originalReq.id, res.id, 'id matches')
      assert.equal(originalReq.jsonrpc, res.jsonrpc, 'jsonrpc version matches')
      assert.equal(res.result, 'saw subengine', 'response was handled by nested engine')
      done()
    })
  })

  it('decorate res', function (done) {
    let engine = new RpcEngine()
    let subengine = new RpcEngine()
    let originalReq

    subengine.push(function (req, res, next, end) {
      originalReq = req
      res.xyz = true
      res.result = true
      end()
    })

    engine.push(asMiddleware(subengine))

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
    let subengine = new RpcEngine()
    let originalReq

    subengine.push(function (req, res, next, end) {
      originalReq = req
      req.xyz = true
      res.result = true
      end()
    })

    engine.push(asMiddleware(subengine))

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
    let subengine = new RpcEngine()

    subengine.push((req, res, next, end) => next())

    engine.push(asMiddleware(subengine))
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

  it('handles next handler correctly when nested', function (done) {
    let engine = new RpcEngine()
    let subengine = new RpcEngine()

    subengine.push((req, res, next, end) => {
      next((cb) => {
        res.copy = res.result
        cb()
      })
    })

    engine.push(asMiddleware(subengine))
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

  it('handles next handler correctly when flat', function (done) {
    let engine = new RpcEngine()
    let subengine = new RpcEngine()

    subengine.push((req, res, next, end) => {
      next((cb) => {
        res.copy = res.result
        cb()
      })
    })

    subengine.push((req, res, next, end) => {
      res.result = true
      end()
    })

    engine.push(asMiddleware(subengine))
    let payload = { id: 1, jsonrpc: '2.0', method: 'hello' }

    engine.handle(payload, function (err, res) {
      assert.ifError(err, 'did not error')
      assert(res, 'has res')
      assert.equal(res.result, res.copy, 'copied result correctly')
      done()
    })
  })
})
