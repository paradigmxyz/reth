var tape = require('tape')

var expected = [
  [[], 'foo'],
  [['foo'], 'bar'],
  [['foo', 'bar'], 'baz'],
  [['foo', 'bar'], 'blerg'],
  [['foobar'], 'barbaz'],
]

//compare two array items
function compare (a, b) {
 if(Array.isArray(a) && Array.isArray(b)) {
    var l = Math.min(a.length, b.length)
    for(var i = 0; i < l; i++) {
      var c = compare(a[i], b[i])
      if(c) return c
    }
    return a.length - b.length
  }
  if('string' == typeof a && 'string' == typeof b)
    return a < b ? -1 : a > b ? 1 : 0

  throw new Error('items not comparable:'
    + JSON.stringify(a) + ' ' + JSON.stringify(b))
}

function random () {
  return Math.random() - 0.5
}

module.exports = function (format) {

  var encoded = expected.map(format.encode)

  tape('ordering', function (t) {

    expected.sort(compare)

    var actual =
      expected.slice()
        .sort(random)
        .map(format.encode)
        .sort()
        .map(format.decode)

    console.log(actual)

    t.deepEqual(actual, expected)

    t.end()
  })


  tape('ranges', function (t) {

    function gt  (a, b, i, j) {
      t.equal(a > b,  i > j,  a + ' gt '  + b + '==' + i >  j)
    }

    function gte (a, b, i, j) {
      t.equal(a >= b, i >= j, a + ' gte ' + b + '==' + i >= j)
    }

    function lt  (a, b, i, j) {
      t.equal(a < b,  i < j,  a + ' lt '  + b + '==' + i <  j)
    }

    function lte (a, b, i, j) {
      t.equal(a <= b, i <= j, a + ' lte ' + b + '==' + i <= j)
    }

    function check(j, cmp) {
      var item = encoded[j]
      for(var i = 0; i < expected.length; i++) {
        //first check less than.
        cmp(item, encoded[i], j, i)
      }
    }

    for(var i = 0; i < expected.length; i++) {
      check(i, gt)
      check(i, gte)
      check(i, lt)
      check(i, lte)
    }

    t.end()
  })
}

module.exports(require('../codec'))

