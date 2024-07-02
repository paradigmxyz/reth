var pull    = require('pull-stream')
var through = require('through')
var toPull  = require('../')

require('tape')('propagate close back to source', function (t) {

  t.plan(1)

  var i = 0

  var ts = through(function (data) {
    console.log(data)
    if(i++ > 100)
      ts.destroy()
  })

  pull(
    pull.infinite(),
    function (read) {
      return function (abort, cb) {
        if(abort) return t.ok(true), t.end()
        read(false, cb)
      }
    },
    toPull(ts),
    pull.drain()
  )

})
