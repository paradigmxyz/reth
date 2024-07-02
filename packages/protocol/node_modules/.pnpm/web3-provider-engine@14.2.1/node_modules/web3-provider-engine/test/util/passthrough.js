const inherits = require('util').inherits
const FixtureProvider = require('../../subproviders/fixture.js')

module.exports = PassthroughProvider

//
// handles no methods, skips all requests
// mostly useless
//

inherits(PassthroughProvider, FixtureProvider)
function PassthroughProvider(methods){
  const self = this
  FixtureProvider.call(self, {})
}
