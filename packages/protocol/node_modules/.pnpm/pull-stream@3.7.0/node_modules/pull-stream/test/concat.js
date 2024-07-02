var test = require('tape')
var pull = require('../')

test('concat', function (t) {
  var n = 0
  pull(
    pull.values('hello there this is a test'.split(/([aeiou])/)),
    pull.through(function () {
      n++
    }), 
    pull.concat(function (err, mess) {
      t.equal(mess, 'hello there this is a test')
      t.equal(n, 17)
      t.end()
    })
  )

})
