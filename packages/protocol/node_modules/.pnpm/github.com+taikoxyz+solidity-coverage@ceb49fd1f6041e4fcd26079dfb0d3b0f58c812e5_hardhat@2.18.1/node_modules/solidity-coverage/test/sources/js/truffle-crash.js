/* eslint-env node, mocha */
/* global artifacts, contract */

var Simple = artifacts.require('Simple');

// This test should break truffle because it has a syntax error.
contract('Simple', () => {
  it('should crash', function(){
    return Simple.new().then.why.
  })
})