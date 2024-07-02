const Duplex = require('readable-stream').Duplex
const inherits = require('util').inherits
const Subprovider = require('./subprovider.js')

module.exports = StreamSubprovider


inherits(StreamSubprovider, Duplex)

function StreamSubprovider(){
  Duplex.call(this, {
    objectMode: true,
  })

  this._payloads = {}
}

StreamSubprovider.prototype.handleRequest = function(payload, next, end){
  var id = payload.id
  // handle batch requests
  if (Array.isArray(payload)) {
    // short circuit for empty batch requests
    if (payload.length === 0){
      return end(null, [])
    }
    id = generateBatchId(payload)
  }
  // store request details
  this._payloads[id] = [payload, end]
  this.push(payload)
}

StreamSubprovider.prototype.setEngine = noop

// stream plumbing

StreamSubprovider.prototype._read = noop

StreamSubprovider.prototype._write = function(msg, encoding, cb){
  this._onResponse(msg)
  cb()
}

// private

StreamSubprovider.prototype._onResponse = function(response){
  var id = response.id
  // handle batch requests
  if (Array.isArray(response)) {
    id = generateBatchId(response)
  }
  var data = this._payloads[id]
  if (!data) throw new Error('StreamSubprovider - Unknown response id')
  delete this._payloads[id]
  var callback = data[1]

  // run callback on empty stack,
  // prevent internal stream-handler from catching errors
  setTimeout(function(){
    callback(null, response.result)
  })
}


// util

function generateBatchId(batchPayload){
  return 'batch:'+batchPayload.map(function(payload){ return payload.id }).join(',')
}

function noop(){}


module.exports = StreamSubprovider
