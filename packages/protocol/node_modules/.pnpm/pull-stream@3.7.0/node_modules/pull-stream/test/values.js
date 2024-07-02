

var pull = require('../')
var tape = require('tape')

tape('values - array', function (t) {
  pull(
    pull.values([1,2,3]),
    pull.collect(function (err, ary) {
      t.notOk(err)
      t.deepEqual(ary, [1, 2, 3])
      t.end()
    })
  )
})

tape('values - object', function (t) {
  pull(
    pull.values({a:1,b:2,c:3}),
    pull.collect(function (err, ary) {
      t.notOk(err)
      t.deepEqual(ary, [1, 2, 3])
      t.end()
    })
  )

})

tape('values, abort', function (t) {

  t.plan(3)

  var err = new Error('intentional')

  var read = pull.values([1,2,3], function (err) {
    t.end()
  })

  read(null, function (_, one) {
    t.notOk(_)
    t.equal(one, 1)
    read(err, function (_err) {
      t.equal(_err, err)
    })
  })

})
