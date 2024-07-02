
var tape = require('tape')
var Duplex = require('../duplex')
var Pair = require('pull-pair')
var pull = require('pull-stream')

tape('simple', function (t) {
  var duplex = Duplex()

  pull(
    pull.values([1,2,3]),
    duplex,
    pull.collect(function (err, values) {
      t.deepEqual(values, [1,2,3])
      t.end()
    })
  )

  //by default, pair gives us a pass through stream as duplex.
  duplex.resolve(Pair())

})


