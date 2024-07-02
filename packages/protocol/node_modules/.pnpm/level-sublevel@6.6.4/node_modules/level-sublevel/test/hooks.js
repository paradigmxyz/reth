var levelup = require('level-test')()

var base = require('../')(levelup('test-sublevels', {valueEncoding: 'json'}))

var test = require('tape')

test('subsections', function (t) {

  var foo = base.sublevel('foo')
  var bar = base.sublevel('bar')

  var n, m, o = m = n = 0
  var q, r = q = 0

  foo.post(function (op) {
    n ++
  })

  //this should do the same
  foo.post({}, function (op) {
    m ++
  })

  foo.post({gte: 'm'}, function (op) {
    o ++
  })

  foo.pre(function (op) {
    t.equal(op.type, 'put')
    q ++
  })

  base.pre(function (op) {
    t.equal(op.type, 'put')
    r ++
  })

  base.batch([
    { key: 'a', value: 1, type: 'put', prefix: foo },
    { key: 'k', value: 2, type: 'put', prefix: foo },
    { key: 'q', value: 3, type: 'put', prefix: foo },
    { key: 'z', value: 4, type: 'put', prefix: foo },
    //into the main base
    { key: 'b', value: 5, type: 'put'},
    { key: 'b', value: 5, type: 'put', prefix: bar}
  ], function (err) {
    t.equal(n, 4)
    t.equal(m, 4)
    t.equal(o, 2)
    t.equal(q, 4)
    t.equal(r, 1)

    t.end()
  })

})


