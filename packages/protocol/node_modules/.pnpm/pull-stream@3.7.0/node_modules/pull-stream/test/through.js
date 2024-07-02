

var pull = require('../')
require('tape')('through - onEnd', function (t) {
  t.plan(2)
  pull(
    pull.infinite(),
    pull.through(null, function (err) {
      if (process.env.TEST_VERBOSE) console.log('end')
      t.ok(true)
      process.nextTick(function () {
        t.end()
      })
    }),
    pull.take(10),
    pull.collect(function (err, ary) {
      if (process.env.TEST_VERBOSE) console.log(ary)
      t.ok(true)
    })
  )
})
