/* eslint-env node, mocha */
/* global artifacts, contract, assert */

const PureView = artifacts.require('PureView');

contract('PureView', accounts => {

  it('calls a pure function', async function(){
    const instance = await PureView.new();
    const value = await instance.isPure(4,5);
  });

  it('calls a view function', async function(){
    const instance = await PureView.new();
    const value = await instance.isView();
  })
});