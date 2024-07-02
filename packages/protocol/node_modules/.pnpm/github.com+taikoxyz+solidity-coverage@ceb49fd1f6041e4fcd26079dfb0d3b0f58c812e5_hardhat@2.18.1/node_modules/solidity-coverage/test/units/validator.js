const assert = require('assert');
const util = require('util');
const ConfigValidator = require('./../../lib/validator');

describe('config validation', () => {
  let validator;
  let solcoverjs;

  before(() => validator = new ConfigValidator());
  beforeEach(() => solcoverjs = {});

  it('validates an empty config', function() {
    assert(validator.validate(solcoverjs), '{} should be valid');
  })

  it('validates config with unknown options', function(){
    solcoverjs.unknown_option = 'hello';
    assert(validator.validate(solcoverjs), '.cwd string should be valid')
  })

  it('validates the "string" options', function(){
    const options =  [
      "cwd",
      "host",
      "istanbulFolder",
      "abiOutputPath",
      "matrixOutputPath",
    ]

    options.forEach(name => {
      // Pass
      solcoverjs = {};
      solcoverjs[name] = "a_string";
      assert(validator.validate(solcoverjs), `${name} string should be valid`)

      // Fail
      solcoverjs[name] = 0;
      try {
        validator.validate(solcoverjs);
        assert.fail()
      } catch (err){
        assert(err.message.includes(`"${name}" is not of a type(s) string`), err.message);
      }
    });
  });

  it('validates the "boolean" options', function(){
    const options =  [
      "silent",
      "autoLaunchServer",
      "measureStatementCoverage",
      "measureFunctionCoverage",
      "measureModifierCoverage",
      "measureBranchCoverage",
      "measureLineCoverage"
    ]

    options.forEach(name => {
      // Pass
      solcoverjs = {};
      solcoverjs[name] = false;
      assert(validator.validate(solcoverjs), `${name} boolean should be valid`)

      // Fail
      solcoverjs[name] = "false";
      try {
        validator.validate(solcoverjs);
        assert.fail()
      } catch (err){
        assert(err.message.includes(`"${name}" is not of a type(s) boolean`), err.message);
      }
    });
  });

  it('validates the "object" options', function(){
    const options =  [
      "client",
      "providerOptions",
    ]

    options.forEach(name => {
      // Pass
      solcoverjs = {};
      solcoverjs[name] = {a_property: 'a'};
      assert(validator.validate(solcoverjs), `${name} object should be valid`)

      // Fail
      solcoverjs[name] = 0;
      try {
        validator.validate(solcoverjs);
        assert.fail()
      } catch (err){
        assert(err.message.includes(`"${name}" is not of a type(s) object`), err.message);
      }
    });
  });

  it('validates the "number" options', function(){
    const options =  [
      "port",
    ]

    options.forEach(name => {
      // Pass
      solcoverjs = {};
      solcoverjs[name] = 0;
      assert(validator.validate(solcoverjs), `${name} number should be valid`)

      // Fail
      solcoverjs[name] = "a_string";
      try {
        validator.validate(solcoverjs);
        assert.fail()
      } catch (err){
        assert(err.message.includes(`"${name}" is not of a type(s) number`), err.message);
      }
    });
  });

  it('validates the "string[]" options', function(){
    const options =  [
      "skipFiles",
      "istanbulReporter",
      "modifierWhitelist"
    ]

    options.forEach(name => {
      // Pass
      solcoverjs = {};
      solcoverjs[name] = ['a_string'];
      assert(validator.validate(solcoverjs), `${name} string array should be valid`)

      // Fail
      solcoverjs[name] = "a_string";
      try {
        validator.validate(solcoverjs);
        assert.fail()
      } catch (err){
        assert(err.message.includes(`"${name}" is not of a type(s) array`), err.message);
      }
    });
  });

  it('validates the "function" options', function(){

    const options =  [
      "onCompileComplete",
      "onServerReady",
      "onTestComplete",
      "onIstanbulComplete",
    ]

    options.forEach(name => {
      // Pass
      solcoverjs = {};
      solcoverjs[name] = async (a,b) => {};
      assert(validator.validate(solcoverjs), `${name} string array should be valid`)

      // Fail
      solcoverjs[name] = "a_string";
      try {
        validator.validate(solcoverjs);
        assert.fail()
      } catch (err){
        assert(err.message.includes(`"${name}" is not a function`), err.message);
      }
    });
  });
});
