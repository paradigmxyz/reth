var tape = require('tape')

var sublevel = require('../')
var level = require('level-test')()

tape('expose version', function (t) {

  t.equal(
    sublevel(level('level-sublevel-ver')).version,
    require('../package.json').version
  )

  t.end()

})
