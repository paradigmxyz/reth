var Live = require('pull-live')

var old = require('./old')
var live = require('./live')

module.exports = function (db, opts) {
  if(opts && opts.tail) {
    console.error('pull-level: .tail option is depreciated. use .live instead')
    opts.live = opts.tail
  }
  return Live(function (opts) {
    return old(db, opts)
  }, function (opts) {
    return live(db, opts)
  })(opts)
}
