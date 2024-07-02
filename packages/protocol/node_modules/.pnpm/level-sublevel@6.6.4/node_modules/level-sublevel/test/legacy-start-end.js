var test = require('tape')

var sublevel = require('../')
var level = require('level-test')()

var mainDB = level('legacy-start-end')
var subDB = sublevel(mainDB).sublevel('test', {valueEncoding: 'json'})

var batch = [
  {key: 'a', value: 1, type: 'put'},
  {key: 'b', value: 2, type: 'put'},
  {key: 'c', value: 3, type: 'put'},
  {key: 'd', value: 4, type: 'put'},
  {key: 'e', value: 5, type: 'put'}
]

function testRange(opts, expected, db, t, cb) {
  var res = []

  db.createReadStream(opts).on('data', function (data) {
    res.push(data.key);
  }).on('end', function () {
    t.deepEqual(res, expected, 'using options: ' + JSON.stringify(opts))
    cb(null)
  }).on('error', cb)
}

// run the same tests for the mainDB and subDB to show
// we have the same behavior as vanilla leveldown

[mainDB, subDB].forEach(function (db, i) {

  var testName = i === 0 ? 'leveldown' : 'sublevel'

  // test a bunch of combinations of start/end with and without reverse
  test('legacy start/end/reverse: ' + testName, function (t) {
    db.batch(batch, function (err) {
      t.notOk(err)

      var i = -1;
      var testCases = [
        [{start: 'b'}, ['b', 'c', 'd', 'e']],
        [{start: 'b', end: 'd'}, ['b', 'c', 'd']],
        [{start: 'a'}, ['a', 'b', 'c', 'd', 'e']],
        [{start: 'e'}, ['e']],
        [{start: '0'}, ['a', 'b', 'c', 'd', 'e']],
        [{start: 'z'}, []],
        [{end: 'c'}, ['a', 'b', 'c']],
        [{end: 'a'}, ['a']],
        [{end: 'e'}, ['a', 'b', 'c', 'd', 'e']],
        [{end: '0'}, []],
        [{end: 'z'}, ['a', 'b', 'c', 'd', 'e']],
        [{start: 'd', end: 'b', reverse: true}, ['d', 'c', 'b']],
        [{start: 'd', reverse: true}, ['d', 'c', 'b', 'a']],
        [{start: 'a', reverse: true}, ['a']],
        [{start: 'e', reverse: true}, ['e', 'd', 'c', 'b', 'a']],
        [{start: '0', reverse: true}, []],
        [{start: 'z', reverse: true}, ['e', 'd', 'c', 'b', 'a']],
        [{end: 'c', reverse: true}, ['e', 'd', 'c']],
        [{end: 'a', reverse: true}, ['e', 'd', 'c', 'b', 'a']],
        [{end: 'e', reverse: true}, ['e']],
        [{end: '0', reverse: true}, ['e', 'd', 'c', 'b', 'a']],
        [{end: 'z', reverse: true}, []],
      ]

      function next(err) {
        t.notOk(err)
        if (++i === testCases.length) {
          return t.end()
        }
        var testCase = testCases[i]
        testRange(testCase[0], testCase[1], db, t, next)
      }

      next()
    })
  })

  // as a sanity check, test exactly the same options, but using the modern
  // lte/gte style instead of start/end
  test('modern start/end/reverse: ' + testName, function (t) {
    db.batch(batch, function (err) {
      t.notOk(err)

      var i = -1;
      var testCases = [
        [{gte: 'b'}, ['b', 'c', 'd', 'e']],
        [{gte: 'b', lte: 'd'}, ['b', 'c', 'd']],
        [{gte: 'a'}, ['a', 'b', 'c', 'd', 'e']],
        [{gte: 'e'}, ['e']],
        [{gte: '0'}, ['a', 'b', 'c', 'd', 'e']],
        [{gte: 'z'}, []],
        [{lte: 'c'}, ['a', 'b', 'c']],
        [{lte: 'a'}, ['a']],
        [{lte: 'e'}, ['a', 'b', 'c', 'd', 'e']],
        [{lte: '0'}, []],
        [{lte: 'z'}, ['a', 'b', 'c', 'd', 'e']],
        [{lte: 'd', gte: 'b', reverse: true}, ['d', 'c', 'b']],
        [{lte: 'd', reverse: true}, ['d', 'c', 'b', 'a']],
        [{lte: 'a', reverse: true}, ['a']],
        [{lte: 'e', reverse: true}, ['e', 'd', 'c', 'b', 'a']],
        [{lte: '0', reverse: true}, []],
        [{lte: 'z', reverse: true}, ['e', 'd', 'c', 'b', 'a']],
        [{gte: 'c', reverse: true}, ['e', 'd', 'c']],
        [{gte: 'a', reverse: true}, ['e', 'd', 'c', 'b', 'a']],
        [{gte: 'e', reverse: true}, ['e']],
        [{gte: '0', reverse: true}, ['e', 'd', 'c', 'b', 'a']],
        [{gte: 'z', reverse: true}, []],
      ]

      function next(err) {
        t.notOk(err)
        if (++i === testCases.length) {
          return t.end()
        }
        var testCase = testCases[i]
        testRange(testCase[0], testCase[1], db, t, next)
      }

      next()
    })
  })
})