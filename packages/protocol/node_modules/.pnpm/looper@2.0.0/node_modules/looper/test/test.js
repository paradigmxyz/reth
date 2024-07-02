
var tape = require('tape')
var looper = require('../')

tape('n=1000000, with no RangeError', function (t) {
  var n = 1000000, c = 0
  looper(function () {
    c ++
    if(--n) return this()
    t.equal(c, 1000000)
    t.end()
  })()
})

tape('async is okay', function (t) {

  var n = 100, c = 0
  looper(function () {
    c ++
    if(--n) return setTimeout(this)
    t.equal(c, 100)
    t.end()
  })()

})

tape('sometimes async is okay', function (t) {
  var i = 1000; c = 0
  looper(function () {
    c++
    if(--i) return Math.random() < 0.1 ? setTimeout(this) : this()
    t.equal(c, 1000)
    t.end()
  })()

})

tape('arguments', function (t) {
  var r = Math.random()

  looper(function (R) {
    t.equal(R, r)
    if(Math.random() > 0.1)
      this(r = Math.random())
    else
      t.end()
  })(r)

})

