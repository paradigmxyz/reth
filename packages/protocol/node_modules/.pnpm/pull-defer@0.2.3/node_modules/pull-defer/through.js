
module.exports = function () {
  var read, reader, cb, abort, stream

  function delayed (_read) {
    //if we already have the stream, go!
    if(stream) return stream(_read)

    read = _read
    return function (_abort, _cb) {
      if(reader) reader(_abort, _cb)
      else abort = _abort, cb = _cb

    }
  }

  delayed.resolve = function (_stream) {
    if(stream) throw new Error('already resolved')
    stream = _stream
    if(!stream) throw new Error('resolve *must* be passed a transform stream')
    if(read) {
      reader = stream(read)
      if(cb) reader(abort, cb)
    }
  }

  return delayed
}
