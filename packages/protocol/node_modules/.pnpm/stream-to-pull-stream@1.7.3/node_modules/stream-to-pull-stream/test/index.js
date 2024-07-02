var pull = require('pull-stream')
var fs = require('fs')
var tape = require('tape')

var toPullStream = require('../')

pull(
  pull.values([
    'hello\n',
    '  there\n'
  ]),
  toPullStream(process.stdout)
)

tape('get end callback even with stdout', function (t) {

  pull(
    toPullStream(fs.createReadStream(__filename)),
    pull.map(function (e) { return e.toString().toUpperCase() }),
    toPullStream.sink(process.stdout, function (err) {
      console.log('----END!')
      t.end()
    })
  )

})
