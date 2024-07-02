
var tape = require('tape')

var range = require('../range')

tape('test prefix', function (t) {

  t.ok( range.prefix([['foo'], 'y'], [['foo'], 'yellow']) )
  t.notOk( range.prefix([['foo'], 'y'], [['foo'], 'Yellow']) )

  t.ok( range.prefix([['foo', 'bar']], [['foo', 'bar'], 'foo']) )
  t.notOk( range.prefix([['foo', 'bar']], [['foo', 'bar', 'baz'], 'foo']) )

  t.ok( range.prefix([[]], [[], 'hello']) )

  t.end()
})


tape('test range', function (t) {

  t.equal(range({lt:  [[], 'apple']}, [[], 'apple']),  false)
  t.equal(range({lte: [[], 'apple']}, [[], 'apple']),  true)
  t.equal(range({gt:  [[], 'apple']}, [[], 'apple']),  false)
  t.equal(range({gte: [[], 'apple']}, [[], 'apple']),  true)

  t.equal(range({lt:  [['A'], 'apple']}, [['A'], 'apple']),  false)
  t.equal(range({lte: [['A'], 'apple']}, [['A'], 'apple']),  true)
  t.equal(range({gt:  [['A'], 'apple']}, [['A'], 'apple']),  false)
  t.equal(range({gte: [['A'], 'apple']}, [['A'], 'apple']),  true)


  t.equal(range({lt:  [[], 'apple']}, [[], 'a']),      true)
  t.equal(range({lt:  [['fruit']]}, [[], 'a']),        true)

  t.equal(range({lte: [[], 'apple']}, [[], 'a']),      true)
  t.equal(range({lte: [[], 'apple']}, [[], 'apples']), false)
  t.equal(range({lte: [[], 'apple']}, [[], 'b']),      false)
  t.equal(range({lte: [[], 'apple']}, [['apples']]),   false)

  t.equal(range({gte: [[], 'apple']}, [[], 'a']),      false)
  t.equal(range({gte: [[], 'apple']}, [[], 'apples']), true)
  t.equal(range({gte: [[], 'apple']}, [[], 'b']),      true)
  t.equal(range({gte: [[], 'apple']}, [['apples']]),   true)

  t.equal(range({gt:  [[], 'apple']}, [[], 'a']),      false)
  t.equal(range({gt:  [[], 'apple']}, [[], 'apples']), true)
  t.equal(range({gt:  [[], 'apple']}, [[], 'b']),      true)
  t.equal(range({gt:  [[], 'apple']}, [['apples']]),   true)

  t.equal(range({lte: [['x']]},      [[], 'x']),         true)
  t.equal(range({lte: [['x'], 'y']}, [['x'], 'x']),      true)
  t.equal(range({lte: [['x', 'z']]}, [['x', 'y'], 'x']), true)

  t.equal(range({lte: [[], 'x']},         [['x']]),      false)
  t.equal(range({lte: [['x'], 'x']},      [['x'], 'y']), false)
  t.equal(range({lte: [['x', 'y'], 'x']}, [['x', 'z']]), false)

  t.equal(range({lt:  [['x']]},      [[], 'x']),         true)
  t.equal(range({lt:  [['x'], 'y']}, [['x'], 'x']),      true)
  t.equal(range({lt:  [['x', 'z']]}, [['x', 'y'], 'x']), true)

  t.equal(range({lt:  [[], 'x']},         [['x']]),      false)
  t.equal(range({lt:  [['x'], 'x']},      [['x'], 'y']), false)
  t.equal(range({lt:  [['x', 'y'], 'x']}, [['x', 'z']]), false)

  t.equal(range({gt:  [['x']]},      [[], 'x']),         false)
  t.equal(range({gt:  [['x'], 'y']}, [['x'], 'x']),      false)
  t.equal(range({gt:  [['x', 'z']]}, [['x', 'y'], 'x']), false)

  t.equal(range({gt:  [[], 'x']},         [['x']]),      true)
  t.equal(range({gt:  [['x'], 'x']},      [['x'], 'y']), true)
  t.equal(range({gt:  [['x', 'y'], 'x']}, [['x', 'z']]), true)

  t.equal(range({gte: [['x']]},      [[], 'x']),         false)
  t.equal(range({gte: [['x'], 'y']}, [['x'], 'x']),      false)
  t.equal(range({gte: [['x', 'z']]}, [['x', 'y'], 'x']), false)

  t.equal(range({gte: [[], 'x']},         [['x']]),      true)
  t.equal(range({gte: [['x'], 'x']},      [['x'], 'y']), true)
  t.equal(range({gte: [['x', 'y'], 'x']}, [['x', 'z']]), true)

  t.end()
})
