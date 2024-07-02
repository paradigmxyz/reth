
//getting stack overflows from writing too fast.
//I think it's because there is a recursive loop
//which stacks up there are many write that don't drain.
//try to reproduce it.
var pull = require('pull-stream')
var through = require('through')
var toPull = require('../')

pull(
  pull.count(1000000),
  pull.map(function (e) {
    return e.toString()+'\n'
  }),
  toPull.sink(through())
)
