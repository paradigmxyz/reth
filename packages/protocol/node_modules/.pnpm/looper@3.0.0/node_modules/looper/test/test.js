
var tape = require('tape')
var looper = require('../')

tape('n=1000000, with no RangeError', function (t) {
  var n = 1000000, c = 0
  looper(function (next) {
    c ++
    if(--n) return next()
    t.equal(c, 1000000)
    t.end()
  })
})

tape('async is okay', function (t) {

  var n = 100, c = 0
  looper(function (next) {
    c ++
    if(--n) return setTimeout(next)
    t.equal(c, 100)
    t.end()
  })

})

tape('sometimes async is okay', function (t) {
  var i = 1000; c = 0
  looper(function (next) {
    c++
    if(--i) return Math.random() < 0.1 ? setTimeout(next) : next()
    t.equal(c, 1000)
    t.end()
  })

})

