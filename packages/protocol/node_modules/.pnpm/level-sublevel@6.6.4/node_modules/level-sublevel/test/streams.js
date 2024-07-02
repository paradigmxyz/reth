var level = require('level-test')()
var sublevel = require('../')

require('tape')('sublevel', function (t) {

  require('rimraf').sync('/tmp/test-sublevel-readstream')

  var db = level('test-sublevel-readstream')
  var base = sublevel(db)

  var a    = base.sublevel('A')

  var i = 0

  function all(db, cb) {
    var o = {}
    db.createReadStream({end: '\xff\xff'}).on('data', function (data) {
      o[data.key.toString()] = data.value.toString()
    })
    .on('end', function () {
      cb(null, o)
    })
    .on('error', cb)
  }

  var _a, _b, _c

  a.batch([
    {key: 'a', value: _a ='AAA_'+Math.random(), type: 'put'},
    {key: 'b', value: _b = 'BBB_'+Math.random(), type: 'put'},
    {key: 'c', value: _c = 'CCC_'+Math.random(), type: 'put'},
  ], function (err) {
    if(err) throw err
    all(db, function (err, obj) {
      console.log(obj)
      t.deepEqual(obj, 
        { '!A!a': _a,
          '!A!b': _b,
          '!A!c': _c
        })

      all(a, function (err, obj) {
        console.log(obj)
        t.deepEqual(obj, 
          { 'a': _a,
            'b': _b,
            'c': _c
          })
        t.end()
      })
    })
  })
})
