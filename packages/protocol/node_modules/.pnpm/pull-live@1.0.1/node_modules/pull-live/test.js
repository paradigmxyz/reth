var pull = require('pull-stream')
var tape = require('tape')
var Live = require('./')

var Pushable = require('pull-pushable')

tape('old only', function (t) {

  var olds = pull.values([1,2,3])

  Live(function () {
    t.ok(true)
    return olds
  }, function () {
    throw new Error('should not be called')
  })({old: true})

  t.end()

})

tape('new only', function (t) {

  var news = Pushable()

  Live(function () {
    throw new Error('should not be called')
  }, function () {
    t.ok(true)
    return news
  })({old: false})

  t.end()

})


tape('old and new', function (t) {

  var olds = pull.values([1,2,3])
  var news = Pushable()
  pull(
    Live(function () {
      return olds
    }, function () {
      t.ok(true)
      return news
    })({live: true, sync: false}),
    pull.collect(function (err, ary) {
      t.deepEqual(ary, [1,2,3,4])
      t.end()
    })
  )

  news.push(4)
  news.end()
})




