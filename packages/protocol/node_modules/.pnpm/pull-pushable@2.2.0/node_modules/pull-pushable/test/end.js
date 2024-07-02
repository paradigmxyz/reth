// var pull = require('pull-stream')
var pushable = require('../')
var test = require('tape')

test('pushable', function (t) {
  var buf = pushable()
  t.plan(10)

  t.equal('function', typeof buf)
  t.equal(2, buf.length)

  buf.push(1)
  buf.push(2)
  buf.push(3)
  buf.end()

  buf(null, function (end, data) {
    t.equal(data, 1)
    t.notOk(end)
    buf(null, function (end, data) {
      t.equal(data, 2)
      t.notOk(end)
      buf(null, function (end, data) {
        t.equal(data, 3)
        t.notOk(end)
        buf(null, function (end, data) {
          t.equal(data, undefined)
          t.ok(end)
          t.end()
        })
      })
    })
  })
})
