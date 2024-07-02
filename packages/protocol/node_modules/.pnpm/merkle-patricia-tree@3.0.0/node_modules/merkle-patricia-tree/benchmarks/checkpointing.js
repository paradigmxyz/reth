var iterations = 500
var samples = 20

var async = require('async')
var crypto = require('crypto')
var Trie = require('../index.js')
var i

function iterTest (numOfIter, cb) {
  var vals = []
  var keys = []

  for (i = 0; i <= numOfIter; i++) {
    vals.push(crypto.pseudoRandomBytes(32))
    keys.push(crypto.pseudoRandomBytes(32))
  }

  var hrstart = process.hrtime()
  var numOfOps = 0
  var trie = new Trie()

  for (i = 0; i < numOfIter; i++) {
    trie.put(vals[i], keys[i], function () {
      trie.checkpoint()
      trie.get('test', function () {
        numOfOps++
        if (numOfOps === numOfIter) {
          var hrend = process.hrtime(hrstart)
          cb(hrend)
        }
      })
    })
  }
}

i = 0
var avg = [0, 0]

async.whilst(function () {
  i++
  return i <= samples
}, function (done) {
  iterTest(iterations, function (hrend) {
    avg[0] += hrend[0]
    avg[1] += hrend[1]

    console.info('Execution time (hr): %ds %dms', hrend[0], hrend[1] / 1000000)
    done()
  })
}, function () {
  console.info('Average Execution time (hr): %ds %dms', avg[0] / samples, avg[1] / 1000000 / samples)
})
