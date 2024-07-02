var pull = require('../')
var tape = require('tape')

tape('keys', function (t) {
  t.plan(2)

  pull(
    pull.keys({a:1, b:2, c:3}),
    pull.collect(function (err, arr) {
      t.notOk(err)
      t.deepEqual(arr, ['a', 'b', 'c'])
    })
  )
})
