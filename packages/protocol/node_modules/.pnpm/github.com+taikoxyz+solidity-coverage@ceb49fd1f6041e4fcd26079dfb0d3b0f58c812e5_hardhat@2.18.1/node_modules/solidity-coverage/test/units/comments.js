const assert = require('assert');
const util = require('./../util/util.js');

describe('comments', () => {

  it('should cover functions even if comments are present immediately after the opening {', () => {
    const info = util.instrumentAndCompile('comments/postFunctionDeclarationComment');
    util.report(info.solcOutput.errors);
  });

  it('should cover lines even if comments are present', () => {
    const info = util.instrumentAndCompile('comments/postLineComment');
    assert.deepEqual([6, 5], info.instrumented.runnableLines);
    util.report(info.solcOutput.errors);
  });

  it('should cover contracts even if comments are present', () => {
    const info = util.instrumentAndCompile('comments/postContractComment');
    util.report(info.solcOutput.errors);
  });

  it('should cover if statements even if comments are present immediately after opening { ', () => {
    const info = util.instrumentAndCompile('comments/postIfStatementComment');
    util.report(info.solcOutput.errors);
  });
});
