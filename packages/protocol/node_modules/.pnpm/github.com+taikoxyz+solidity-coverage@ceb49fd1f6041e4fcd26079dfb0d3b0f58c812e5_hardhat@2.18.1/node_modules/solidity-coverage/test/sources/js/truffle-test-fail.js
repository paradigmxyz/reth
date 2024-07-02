/* eslint-env node, mocha */
/* global artifacts, contract, assert */

const Simple = artifacts.require('Simple');

contract('Simple', () => {
  it('should set x to 5', async function() {
    let simple = await Simple.new();
    await simple.test(5);
    const val = await simple.getX();
    assert.equal(val.toNumber(), 4) // <-- Wrong result: test fails
  });

  it('should set x to 2', async function() {
    let simple = await Simple.new();
    await simple.test(5);
    const val = await simple.getX();
    assert.equal(val.toNumber(), 2) // <-- Wrong result: test fails
  });
});
