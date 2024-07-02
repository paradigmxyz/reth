var pull    = require('pull-stream')
var through = require('through')
var toPull  = require('../')

var stream  = require('stream')

if (stream.Readable) {
  require('tape')('issue-3', function (t) {
    var util = require('util')
    util.inherits(Counter, stream.Readable)

    function Counter() {
      stream.Readable.call(this, {objectMode: true, highWaterMark: 1})
      this._max = 5
      this._index = 1
    }

    Counter.prototype._read = function() {
      var i = this._index++
      this.push(i)
      if (i >= this._max) this.push(null)
    };

    pull(
      toPull(new Counter()),
      pull.asyncMap(function (value, done) {
        process.nextTick(function() {
          done(null, value)
        })
      }),
      pull.collect(function (err, values) {
        t.deepEqual(values, [1, 2, 3, 4, 5])
        t.end()
      })
    )

  })
}
