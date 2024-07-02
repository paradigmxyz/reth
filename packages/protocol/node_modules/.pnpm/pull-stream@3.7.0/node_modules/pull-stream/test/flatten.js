var pull = require('../')
var test = require('tape')

test('flatten arrays', function (t) {
  pull(
    pull.values([
      [1, 2, 3],
      [4, 5, 6],
      [7, 8, 9]
    ]),
    pull.flatten(),
    pull.collect(function (err, numbers) {
      t.deepEqual([1, 2, 3, 4, 5, 6, 7, 8, 9], numbers)
      t.end()
    })
  )
})

test('flatten - number of reads', function (t) {
  var reads = 0
  pull(
    pull.values([
      pull.values([1, 2, 3]),
    ]),
    pull.flatten(),
    pull.through(function() {
      reads++
      if (process.env.TEST_VERBOSE) console.log('READ', reads)
    }),
    pull.take(2),
    pull.collect(function (err, numbers) {
      t.deepEqual([1, 2], numbers)
      t.equal(reads, 2)
      t.end()
    })
  )

})
test('flatten stream of streams', function (t) {

  pull(
    pull.values([
      pull.values([1, 2, 3]),
      pull.values([4, 5, 6]),
      pull.values([7, 8, 9])
    ]),
    pull.flatten(),
    pull.collect(function (err, numbers) {
      t.deepEqual([1, 2, 3, 4, 5, 6, 7, 8, 9], numbers)
      t.end()
    })
  )

})

test('flatten stream of broken streams', function (t) {
  var _err = new Error('I am broken'), sosEnded
  pull(
    pull.values([
      pull.error(_err)
    ], function(err) {
      sosEnded = err;
    }),
    pull.flatten(),
    pull.onEnd(function (err) {
      t.equal(err, _err)
      process.nextTick(function() {
        t.equal(sosEnded, null, 'should abort stream of streams')
        t.end()
      })
    })
  )
})

test('abort flatten', function (t) {
  var sosEnded, s1Ended, s2Ended
  var read = pull(
    pull.values([
      pull.values([1,2], function(err) {s1Ended = err}),
      pull.values([3,4], function(err) {s2Ended = err}),
    ], function(err) {
      sosEnded = err;
    }),
    pull.flatten()
  )

  read(null, function(err, data) {
    t.notOk(err)
    t.equal(data,1)
    read(true, function(err, data) {
      t.equal(err, true)
      process.nextTick(function() {
        t.equal(sosEnded, null, 'should abort stream of streams')
        t.equal(s1Ended, null, 'should abort current nested stream')
        t.equal(s2Ended, undefined, 'should not abort queued nested stream')
        t.end()
      })
    })
  })
})

test('abort flatten before 1st read', function (t) {
  var sosEnded, s1Ended
  var read = pull(
    pull.values([
      pull.values([1,2], function(err) {s1Ended = err})
    ], function(err) {
      sosEnded = err;
    }),
    pull.flatten()
  )

  read(true, function(err, data) {
    t.equal(err, true)
    t.notOk(data)
    process.nextTick(function() {
      t.equal(sosEnded, null, 'should abort stream of streams')
      t.equal(s1Ended, undefined, 'should abort current nested stream')
      t.end()
    })
  })
})

test('flattern handles stream with normal objects', function (t) {
  pull(
    pull.values([
      [1,2,3], 4, [5,6,7], 8, 9 ,10
    ]),
    pull.flatten(),
    pull.collect(function (err, ary) {
      t.deepEqual(ary, [1,2,3,4,5,6,7,8,9,10])
      t.end()
    })
  )
})
