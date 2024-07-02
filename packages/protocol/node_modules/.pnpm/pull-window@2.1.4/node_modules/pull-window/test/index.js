var pull = require('pull-stream')
var window = require('../')

var all = []

require('tape')('window', function (t) {

  pull(
    (function () {
    var i = 0

      return function (abort, cb) {
        if(abort) return cb(true)
        setTimeout(function () {
          cb(null, i++)
        }, Math.random() * 75)
      }
    })(),
    pull.take(50),
    pull.through(function (e) {
      all.push(e)
    }),
    window.recent(20, 200),
    pull.through(console.log),
    pull.collect(function (err, ary) {
      t.notOk(err)
      ary.forEach(function (e) {
        t.ok(Array.isArray(e))
      })
      t.deepEqual(ary.reduce(function (acc, item) {
        return acc.concat(item)
      }, []), all)
      t.end()
    })
  )

})
