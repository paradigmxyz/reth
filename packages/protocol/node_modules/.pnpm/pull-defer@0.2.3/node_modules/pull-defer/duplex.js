
var Source = require('./source')
var Sink = require('./sink')

module.exports = function () {

  var source = Source()
  var sink = Sink()

  return {
    source: source,
    sink: sink,
    resolve: function (duplex) {
      source.resolve(duplex.source)
      sink.resolve(duplex.sink)

    }
  }


}
