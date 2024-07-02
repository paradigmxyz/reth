const createPayload = require('../util/create-payload.js')

module.exports = SubProvider

// this is the base class for a subprovider -- mostly helpers


function SubProvider() {

}

SubProvider.prototype.setEngine = function(engine) {
  const self = this
  if (self.engine) return
  self.engine = engine
  engine.on('block', function(block) {
    self.currentBlock = block
  })

  engine.on('start', function () {
    self.start()
  })

  engine.on('stop', function () {
    self.stop()
  })
}

SubProvider.prototype.handleRequest = function(payload, next, end) {
  throw new Error('Subproviders should override `handleRequest`.')
}

SubProvider.prototype.emitPayload = function(payload, cb){
  const self = this
  self.engine.sendAsync(createPayload(payload), cb)
}

// dummies for overriding

SubProvider.prototype.stop = function () {}

SubProvider.prototype.start = function () {}
