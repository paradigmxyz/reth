// var pull = require('pull-stream')
var tape = require('tape')
var Pushable = require('../')

tape('abort after a read', function (t) {
  t.plan(2)
  var _err = new Error('test error')
  var p = Pushable(function (err) {
    console.log('on close')
    t.equal(err, _err)
  })

  // manual read.
  p(null, function (err, data) {
    console.log('read cb')
    t.equal(err, _err)
  })

  p(_err, function () {
    console.log('abort cb')
    t.end()
  })
})

tape('abort without a read', function (t) {
  t.plan(1)
  var _err = new Error('test error')
  var p = Pushable(function (err) {
    console.log('on close')
    t.equal(err, _err)
  })

  p(_err, function () {
    console.log('abort cb')
    t.end()
  })
})

tape('abort without a read, with data', function (t) {
  t.plan(1)
  var _err = new Error('test error')
  var p = Pushable(function (err) {
    console.log('on close')
    t.equal(err, _err)
  })

  p(_err, function () {
    console.log('abort cb')
    t.end()
  })

  p.push(1)
})
