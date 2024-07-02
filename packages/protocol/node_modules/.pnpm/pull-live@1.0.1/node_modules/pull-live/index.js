var pull = require('pull-stream/pull')
var Cat = require('pull-cat')
var Once = require('pull-stream/sources/once')

module.exports = function (createSource, createLive) {

  return function (opts) {
      opts = opts || {}
      var isOld = opts.old !== false
      var isLive = opts.live === true || opts.old === false

      if(!isLive && !isOld)
        throw new Error('ls with neither old or new is empty')

      if(isLive && isOld)
        return Cat([
          createSource(opts),
          opts.sync === false ? null : Once({sync: true}),
          createLive(opts)
        ])
      else if(!isLive)
        return createSource(opts)
      else
        return createLive(opts)
  }
}






