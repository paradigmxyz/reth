/* eslint-env mocha */
'use strict'

const assert = require('assert')
const RpcEngine = require('../src/index.js')
const createAsyncMiddleware = require('../src/createAsyncMiddleware.js')

describe('createAsyncMiddleware tests', function () {
  it('basic middleware test', function (done) {
    let engine = new RpcEngine()

    engine.push(createAsyncMiddleware(async (req, res, next) => {
      res.result = 42
    }))

    let payload = { id: 1, jsonrpc: '2.0', method: 'hello' }

    engine.handle(payload, function (err, res) {
      assert.ifError(err, 'did not error')
      assert(res, 'has res')
      assert.equal(res.result, 42, 'has expected result')
      done()
    })
  })

  it('next middleware test', function (done) {
    let engine = new RpcEngine()

    engine.push(createAsyncMiddleware(async (req, res, next) => {
      assert.ifError(res.result, 'does not have result')
      await next()
      assert.equal(res.result, 1234, 'value was set as expected')
      // override value
      res.result = 42
    }))

    engine.push(function (req, res, next, end) {
      res.result = 1234
      end()
    })

    let payload = { id: 1, jsonrpc: '2.0', method: 'hello' }

    engine.handle(payload, function (err, res) {
      assert.ifError(err, 'did not error')
      assert(res, 'has res')
      assert.equal(res.result, 42, 'has expected result')
      done()
    })
  })

  it('basic throw test', function (done) {
    let engine = new RpcEngine()

    const error = new Error('bad boy')

    engine.push(createAsyncMiddleware(async (req, res, next) => {
      throw error
    }))

    let payload = { id: 1, jsonrpc: '2.0', method: 'hello' }

    engine.handle(payload, function (err, res) {
      assert(err, 'has err')
      assert.equal(err, error, 'has expected result')
      done()
    })
  })

  it('throw after next test', function (done) {
    let engine = new RpcEngine()

    const error = new Error('bad boy')

    engine.push(createAsyncMiddleware(async (req, res, next) => {
      await next()
      throw error
    }))

    engine.push(function (req, res, next, end) {
      res.result = 1234
      end()
    })

    let payload = { id: 1, jsonrpc: '2.0', method: 'hello' }

    engine.handle(payload, function (err, res) {
      assert(err, 'has err')
      assert.equal(err, error, 'has expected result')
      done()
    })
  })

  it('doesnt await next', function (done) {
    let engine = new RpcEngine()

    engine.push(createAsyncMiddleware(async (req, res, next) => {
      next()
    }))

    engine.push(function (req, res, next, end) {
      res.result = 1234
      end()
    })

    let payload = { id: 1, jsonrpc: '2.0', method: 'hello' }

    engine.handle(payload, function (err, res) {
      assert.ifError(err, 'has err')
      done()
    })
  })
})
