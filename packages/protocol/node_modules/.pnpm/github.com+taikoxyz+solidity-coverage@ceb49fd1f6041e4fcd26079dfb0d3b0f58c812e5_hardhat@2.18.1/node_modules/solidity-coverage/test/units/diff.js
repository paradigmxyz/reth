const assert = require('assert');
const util = require('./../util/util.js');
const Api = require('./../../lib/api')

describe('abi diffs', function(){
  const api = new Api();

  function setUp(source){
    const abis = util.getDiffABIs(source);
    const orig = api.abiUtils.generateHumanReadableAbiList([abis.original], abis.original.sha);
    const cur = api.abiUtils.generateHumanReadableAbiList([abis.current], abis.current.sha);
    return api.abiUtils.diff(orig[0], cur[0]);
  }

  function validate(result, expectPlus, expectMinus, expectDiff){
    assert.equal(result.plus, expectPlus);
    assert.equal(result.minus, expectMinus);
    assert.deepEqual(result.unifiedDiff, expectDiff);
  }

  it('when methods are added', function() {
    const expectPlus = 1;
    const expectMinus = 0;
    const expectDiff = [
      "--- Test\tsha: d8b26d8",
      "+++ Test\tsha: e77e29d",
      "@@ -1,4 +1,5 @@",
      " function a() nonpayable",
      " function b() nonpayable",
      " function c() nonpayable",
      "+function d() nonpayable",
      " function y() view returns (uint256)"
    ];

    validate(setUp('diff/addition'), expectPlus, expectMinus, expectDiff);
  });

  it('when there are events', function() {
    const expectPlus = 2;
    const expectMinus = 1;
    const expectDiff = [
      "--- Test\tsha: d8b26d8",
      "+++ Test\tsha: e77e29d",
      "@@ -1,2 +1,3 @@",
      " function a() nonpayable",
      "-event Evt(uint256,bytes8)",
      "+event _Evt(bytes8,bytes8)",
      "+event aEvt(bytes8)"
    ];

    validate(setUp('diff/events'), expectPlus, expectMinus, expectDiff);
  });

  it('when parameters change', function() {
    const expectPlus = 2;
    const expectMinus = 2;
    const expectDiff = [
      "--- Test\tsha: d8b26d8",
      "+++ Test\tsha: e77e29d",
      "@@ -1,3 +1,3 @@",
      " function a() nonpayable",
      "-function b() nonpayable",
      "-function c() nonpayable",
      "+function b(bytes8) nonpayable",
      "+function c(uint256,uint256) nonpayable"
    ];

    validate(setUp('diff/param-change'), expectPlus, expectMinus, expectDiff);
  });

  it('when there is no change', function() {
    const expectPlus = 0;
    const expectMinus = 0;
    const expectDiff = [];
    validate(setUp('diff/no-change'), expectPlus, expectMinus, expectDiff);
  });

  it('when methods are removed', function() {
    const expectPlus = 0;
    const expectMinus = 1;
    const expectDiff = [
      '--- Test\tsha: d8b26d8',
      '+++ Test\tsha: e77e29d',
      '@@ -1,3 +1,2 @@',
      ' function a() nonpayable',
      ' function b() nonpayable',
      '-function c() nonpayable'
    ];

    validate(setUp('diff/removal'), expectPlus, expectMinus, expectDiff);
  });

  it('when methods are reordered', function() {
    const expectPlus = 0;
    const expectMinus = 0;
    const expectDiff = [];
    validate(setUp('diff/reorder'), expectPlus, expectMinus, expectDiff);
  });

  it('when return signatures change', function() {
    const expectPlus = 4;
    const expectMinus = 1;
    const expectDiff = [
      '--- Test\tsha: d8b26d8',
      '+++ Test\tsha: e77e29d',
      '@@ -1 +1,4 @@',
      '-function a() view returns (uint256)',
      '+function a() view returns (bool)',
      '+function e() view returns (uint8[2])',
      '+function f() view returns (uint8[2], uint256)',
      '+function g() view returns (uint8[3])'
    ];
    validate(setUp('diff/return-sig'), expectPlus, expectMinus, expectDiff);
  });

  it('when state modifiablility changes', function() {
    const expectPlus = 1;
    const expectMinus = 1;
    const expectDiff = [
      '--- Test\tsha: d8b26d8',
      '+++ Test\tsha: e77e29d',
      '@@ -1 +1 @@',
      '-function a() nonpayable',
      '+function a() view returns (bool)'
    ];
    validate(setUp('diff/state-mod-change'), expectPlus, expectMinus, expectDiff);
  });
});
