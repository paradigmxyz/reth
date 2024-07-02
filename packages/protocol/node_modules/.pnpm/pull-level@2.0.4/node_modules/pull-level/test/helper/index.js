var pull = require('pull-stream')
var l    = require('../../')
var timestamp = require('monotonic-timestamp')

exports.random = 
function (db, n, cb) {
  var all = []

  pull(
    pull.infinite(),
    pull.map(function (e) {
      return {
        key: e.toString(),
        value: new Date().toString()
      }
    }),
    pull.take(n),
    pull.through(function (e) {
      all.push({key:e.key, value: e.value})
    }),
    l.write(db, function (err) {
     cb(err, all)
    })
  )
}

exports.sort = function (all) {
  return all.sort(function (a, b) {
      return a.key === b.key ? 0 : a.key < b.key ? -1 : 1
    })
}


exports.words = function (db, cb) {
  var all
  pull(
    pull.values(all = [
      {key: 'A', value: 'apple'},
      {key: 'B', value: 'banana'},
      {key: 'C', value: 'cherry'},
      {key: 'D', value: 'durian'},
      {key: 'E', value: 'elder-berry'},
    ]),
    l.write(db, function (err) {
      console.log('ALL', err, all)
      cb(err, all)
    })
  )
}


var ts = 0
exports.timestamps = function (db, n, cb) {
  var all = []
  ended = false
  pull(
    pull.infinite(),
    pull.take(n),
    pull.map(function (e) {
      return {
        key   : timestamp().toString(),
        value : e.toString()
      }
    }),
    pull.through(function (e) {
      all.push({key:e.key, value: e.value})
    }),
    l.write(db, function (err) {
      if(ended) throw new Error('ended twice')
      ended = true
      cb(err, all)
    })
  )

}

exports.exactly = function (n, err) {
  return function (read) {
    return function (abort, cb) {
      if(0 <=--n) read(abort, cb)
      else console.log(abort, err, true), read(abort || err || true, function (end, data) {
        console.log('ABORTED', end, data)
        cb(end, data)
      })
    }
  }
}

