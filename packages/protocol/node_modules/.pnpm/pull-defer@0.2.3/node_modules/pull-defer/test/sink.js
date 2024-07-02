var tape = require('tape')
var pull = require('pull-stream')
var lazy = require('../sink')

tape('simple', function (t) {

  var feed = [], l

  pull(
    pull.values(feed),
    l = lazy(pull.collect(function (err, ary) {
      if(err) throw err
      t.deepEqual(ary, [1, 2, 3])
      t.end()
    }))
  )

  setTimeout(function () {
    feed.push(1, 2, 3)
    l.start()
  })
})

tape('simple - set late', function (t) {

  var feed = [], l

  pull(pull.values(feed), l = lazy())

  setTimeout(function () {
    feed.push(1, 2, 3)

    l.start(pull.collect(function (err, ary) {
      if(err) throw err
      t.deepEqual(ary, [1, 2, 3])
      t.end()
    }))
  })
})

