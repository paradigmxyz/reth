
module.exports = injectSubproviderMetrics

function injectSubproviderMetrics(subprovider){
  subprovider.getWitnessed = getWitnessed.bind(subprovider)
  subprovider.getHandled = getHandled.bind(subprovider)
  subprovider.clearMetrics = () => {
    subprovider.payloadsWitnessed = {}
    subprovider.payloadsHandled = {}
  }

  subprovider.clearMetrics()

  var _super = subprovider.handleRequest.bind(subprovider)
  subprovider.handleRequest = handleRequest.bind(subprovider, _super)

  return subprovider
}

function getWitnessed(method){
  const self = this
  var witnessed = self.payloadsWitnessed[method] = self.payloadsWitnessed[method] || []
  return witnessed
}

function getHandled(method){
  const self = this
  var witnessed = self.payloadsHandled[method] = self.payloadsHandled[method] || []
  return witnessed
}

function handleRequest(_super, payload, next, end){
  const self = this
  // mark payload witnessed
  var witnessed = self.getWitnessed(payload.method)
  witnessed.push(payload)
  // continue
  _super(payload, next, function(err, result){
    // mark payload handled
    var handled = self.getHandled(payload.method)
    handled.push(payload)
    // continue
    end(err, result)
  })
}