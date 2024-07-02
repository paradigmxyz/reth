const assert = require('assert');
const util = require('./../util/util.js');

describe('generic expressions / misc', () => {
  it('should compile after instrumenting a single binary expression', () => {
    const info = util.instrumentAndCompile('expressions/single-binary-expression');
    util.report(info.solcOutput.errors);
  });

  it('should compile after instrumenting a new expression', () => {
    const info = util.instrumentAndCompile('expressions/new-expression');
    util.report(info.solcOutput.errors);
  });

  it('should compile after instrumenting function that returns true', () => {
    const info = util.instrumentAndCompile('return/return');
    util.report(info.solcOutput.errors);
  });

  it('should compile after instrumenting function that returns void', () => {
    const info = util.instrumentAndCompile('return/empty-return');
    util.report(info.solcOutput.errors);
  });

  it('should compile after instrumenting function that returns via ternary conditional', () => {
    const info = util.instrumentAndCompile('return/ternary-return');
    util.report(info.solcOutput.errors);
  });
});
