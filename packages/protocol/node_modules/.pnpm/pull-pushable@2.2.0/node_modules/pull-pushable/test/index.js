var pull = require('pull-stream')
var pushable = require('../')
var test = require('tape')

test('pushable', function (t) {
  var buf = pushable()

  // should be a read function!

  t.equal('function', typeof buf)
  t.equal(2, buf.length)

  buf.push(1)

  t.deepEqual(buf.buffer, [1])

  pull(
    buf,
    pull.collect(function (end, array) {
      console.log(array)
      t.deepEqual(array, [1, 2, 3])
      t.end()
    })
  )

  // SOMETIMES YOU NEED PUSH!

  buf.push(2)
  buf.push(3)
  buf.end()
})

test('pushable with separated functions', function (t) {
  t.plan(4)

  var { push, end, source, buffer } = pushable(true)

  t.is(typeof source, 'function', 'is a function')
  t.is(source.length, 2, 'is a source stream')

  push(1)
  push(2)

  t.deepEqual(buffer, [1, 2])

  pull(
    source,
    pull.collect(function (err, data) {
      if (err) throw err
      console.log(data)
      t.same(data, [1, 2, 3], 'got expected output')
    })
  )

  push(3)
  end()
})
