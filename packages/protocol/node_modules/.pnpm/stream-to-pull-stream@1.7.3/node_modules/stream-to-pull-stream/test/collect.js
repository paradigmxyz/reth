var pull    = require('pull-stream')
var through = require('through')
var toPull  = require('../')


require('tape')('collect', function (t) {

  var values = [.1, .4, .6, 0.7, .94]
  pull(
    pull.values(values),
    toPull(through()),
    pull.collect(function (err, _values) {
      t.deepEqual(_values, values)
      t.end()
    })
  )

})
