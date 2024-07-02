const assert = require('assert')
const { ConfigMissingError } = require('../../lib/common/errors')

describe('errors', () => {
  it('should throw a ConfigMissingError and match the message', () => {
    const ThrowError = () => {
      throw new ConfigMissingError('config_example.json')
    }

    assert.throws(ThrowError, ConfigMissingError)
    assert.throws(ThrowError, /Failed to load config "config_example.json" to extend from.$/)
  })

  it('should throw a ConfigMissingError and match the default message', () => {
    const ThrowError = () => {
      throw new ConfigMissingError()
    }

    assert.throws(ThrowError, ConfigMissingError)
    assert.throws(ThrowError, /Failed to load a solhint's config file.$/)
  })
})
