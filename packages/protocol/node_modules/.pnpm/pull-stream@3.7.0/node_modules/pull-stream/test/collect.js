var pull = require('../')
var test = require('tape')

test('collect empty', function (t) {
  pull(
    pull.empty(),
    pull.collect(function (err, ary) {
      t.notOk(err)
      t.deepEqual(ary, [])
      t.end()
    })
  )
})
