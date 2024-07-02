var pushable = require('pull-pushable')
var cat      = require('pull-cat')
var post     = require('level-post')

module.exports = function (db, opts) {
  opts = opts || {}

  var l = pushable(function (err) {
    if(opts.onAbort) opts.onAbort(err)
    cleanup()
  })

  var cleanup = post(db, opts, function (ch) {
    if(opts.keys === false)
      l.push(ch.value)
    else if(opts.values === false)
      l.push(ch.key)
    else
      l.push(ch)
  })

  return l

}

