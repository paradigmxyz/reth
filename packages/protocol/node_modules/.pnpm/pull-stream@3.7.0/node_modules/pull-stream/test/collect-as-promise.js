var pull = require('../')
var test = require('tape')

test('collectAsPromise on values', async (t) => {
  const ary = await pull(
    pull.values([10,20,30]),
    pull.collectAsPromise()
  )
  t.deepEquals(ary, [10,20,30])
})
