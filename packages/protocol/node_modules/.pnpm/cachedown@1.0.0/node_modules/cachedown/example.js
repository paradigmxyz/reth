var slow = require('memdown')
var fast = require('./')
var levelup = require('levelup')

var db = levelup('./db.db', {
  db: function (location) {
    // careful! this db has a max cache size of Infinity!
    // to limit cache size, use fast(location, slow).maxSize(/* max size */)
    return fast(location, slow)
  }
})

// use db with better performance for puts and gets
db.put('hey', 'ho', function (err) {
  db.get('hey', function (err, val) {
    // val comes from internal cache
    console.log(val) // ho
  })
})
