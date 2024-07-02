var pull = require('../pull')
var count = require('../sources/count')
var error = require('../sources/error')
var map = require('../throughs/map')
var test = require('tape')

test('continuable stream', function (t) {
  t.plan(2)

  var continuable = function (read) {
    return function (cb) {
      read(null, function  next (end, data) {
        if (end === true) return cb(null)
        if (end) return cb(end)
        read(end, next)
      })
    }
  }

  // With values:
  pull(
    count(5),
    map(function (item) {
      return item * 2
    }),
    continuable
  )(function (err) {
    t.false(err, 'no error')
  })

  // With error:
  pull(
    error(new Error('test error')),
    continuable
  )(function (err) {
    t.is(err.message, 'test error', 'error')
  })
})
