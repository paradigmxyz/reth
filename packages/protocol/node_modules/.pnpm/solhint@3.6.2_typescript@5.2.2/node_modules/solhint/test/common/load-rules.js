const assert = require('assert')
const _ = require('lodash')
const { loadRule, loadRules } = require('../../lib/load-rules')

describe('Load Rules', () => {
  it('should load all rules', () => {
    const rules = loadRules()

    for (const rule of rules) {
      assert.equal(typeof rule, 'object')
      assert.equal(_.has(rule, 'meta'), true)
      assert.equal(_.has(rule, 'ruleId'), true)
      assert.equal(_.has(rule.meta, 'type'), true)
      assert.equal(_.has(rule.meta, 'docs'), true)
      assert.equal(_.has(rule.meta, 'isDefault'), true)
      assert.equal(_.has(rule.meta, 'recommended'), true)
      assert.equal(_.has(rule.meta, 'defaultSetup'), true)
      assert.equal(_.has(rule.meta, 'schema'), true)
    }
  })

  it('should load a single rule', () => {
    const rule = loadRule('func-param-name-mixedcase')

    assert.equal(typeof rule, 'object')
    assert.equal(_.has(rule, 'meta'), true)
    assert.equal(_.has(rule, 'ruleId'), true)
    assert.equal(_.has(rule.meta, 'type'), true)
    assert.equal(_.has(rule.meta, 'docs'), true)
    assert.equal(_.has(rule.meta, 'isDefault'), true)
    assert.equal(_.has(rule.meta, 'recommended'), true)
    assert.equal(_.has(rule.meta, 'defaultSetup'), true)
    assert.equal(_.has(rule.meta, 'schema'), true)
  })

  it('should not load a single rule', () => {
    const rule = loadRule('foo')

    assert.equal(_.isUndefined(rule), true)
  })
})
