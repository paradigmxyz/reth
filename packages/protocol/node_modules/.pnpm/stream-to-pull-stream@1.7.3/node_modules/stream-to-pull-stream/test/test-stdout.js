

//i saw bugs with streams to stdout not ending correctly.
//if there was a pause, it would not.


var cp = require('child_process')
var toPull = require('../')
var pull = require('pull-stream')
var split = require('pull-split')


console.log(process.execPath, [require.resolve('./stdout')])
var child = cp.spawn(process.execPath, [require.resolve('./stdout')])
child.on('exit', function () {
  console.log('ended')
})
pull(
  toPull.source(child.stdout),
  split('\n\n'),
  pull.filter(),
  pull.map(function (e) {
    try {
      return JSON.parse(e)
    } catch (err) {
      console.log(JSON.stringify(e))
      //throw err
    }

  }),
  pull.asyncMap(function (data, cb) {
    setTimeout(function () {
      cb(null, data)
    }, 10)
  }),
  pull.drain(null, function (err) {
    console.log('DONE')
    if(err) throw err
    console.log('done')
  })
)

