const inherits = require('util').inherits
const Subprovider = require('../../subproviders/subprovider.js')
const extend = require('xtend')

module.exports = MockSubprovider

inherits(MockSubprovider, Subprovider)

function MockSubprovider(handleRequest){
  const self = this

  // Optionally provide a handleRequest method
  if (handleRequest) {
    this.handleRequest = handleRequest
  }
}

var mockResponse = {
  data: 'mock-success!'
}
MockSubprovider.prototype.handleRequest = function(payload, next, end){
  end(mockResponse)
}

