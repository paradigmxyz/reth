const assert = require('assert');
const util = require('./../util/util.js');

describe('assembly expressions', () => {

  it('should compile after instrumenting an assembly function with spaces in parameters', () => {
    const info = util.instrumentAndCompile('assembly/spaces-in-function');
    util.report(info.solcOutput.errors);
  });

  it('should compile after instrumenting an assembly if statement', () => {
    const info = util.instrumentAndCompile('assembly/if');
    util.report(info.solcOutput.errors);
  });

});
