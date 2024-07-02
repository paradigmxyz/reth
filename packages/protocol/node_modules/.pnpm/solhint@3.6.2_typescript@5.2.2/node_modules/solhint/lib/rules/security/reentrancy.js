const _ = require('lodash')
const BaseChecker = require('../base-checker')
const TreeTraversing = require('../../common/tree-traversing')

const { hasMethodCalls, findPropertyInParents } = TreeTraversing

const ruleId = 'reentrancy'
const meta = {
  type: 'security',

  docs: {
    description: `Possible reentrancy vulnerabilities. Avoid state changes after transfer.`,
    category: 'Security Rules',
    examples: {
      good: [
        {
          description: 'Invulnerable Contract 1',
          code: require('../../../test/fixtures/security/reentrancy-invulenarble')[0],
        },
        {
          description: 'Invulnerable Contract 2',
          code: require('../../../test/fixtures/security/reentrancy-invulenarble')[1],
        },
        {
          description: 'Invulnerable Contract 3',
          code: require('../../../test/fixtures/security/reentrancy-invulenarble')[2],
        },
      ],
      bad: [
        {
          description: 'Vulnerable Contract 1',
          code: require('../../../test/fixtures/security/reentrancy-vulenarble')[0],
        },
        {
          description: 'Vulnerable Contract 2',
          code: require('../../../test/fixtures/security/reentrancy-vulenarble')[1],
        },
      ],
    },
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class ReentrancyChecker extends BaseChecker {
  constructor(reporter, inputSrc) {
    super(reporter, ruleId, meta)
    this.inputSrc = inputSrc
  }

  ContractDefinition(node) {
    node.stateDeclarationScope = new StateDeclarationScope()
    const scope = node.stateDeclarationScope

    new ContractDefinition(node).stateDefinitions().forEach((i) => scope.trackStateDeclaration(i))
  }

  FunctionDefinition(node) {
    node.effects = new Effects(StateDeclarationScope.of(node))
  }

  ExpressionStatement(node) {
    this._checkAssignment(node)
  }

  MemberAccess(node) {
    this._checkSendCall(node)
  }

  _checkAssignment(node) {
    const assignOperator = AssignOperator.of(node, this.inputSrc)
    const effects = Effects.of(node)

    if (assignOperator && effects && effects.isNotAllowed(assignOperator)) {
      this._warn(node)
    }
  }

  _checkSendCall(node) {
    if (hasMethodCalls(node, ['send', 'transfer'])) {
      Effects.of(node).trackTransfer()
    }
  }

  _warn(ctx) {
    this.warn(ctx, 'Possible reentrancy vulnerabilities. Avoid state changes after transfer.')
  }
}

class Effects {
  static of(node) {
    return findPropertyInParents(node, 'effects')
  }

  constructor(statesScope) {
    this.states = statesScope && statesScope.states
    this.hasTransfer = false
  }

  isNotAllowed(operator) {
    return this.hasTransfer && operator.modifyOneOf(this.states)
  }

  trackTransfer() {
    this.hasTransfer = true
  }
}

class StateDeclarationScope {
  static of(node) {
    return findPropertyInParents(node, 'stateDeclarationScope')
  }

  constructor() {
    this.states = []
  }

  trackStateDeclaration(stateDefinition) {
    const stateName = stateDefinition.stateName()
    this.states.push(stateName)
  }
}

class ContractDefinition {
  constructor(node) {
    this.node = node
  }

  stateDefinitions() {
    return this.node.subNodes
      .map((i) => new ContractPart(i))
      .filter((i) => i.isStateDefinition())
      .map((i) => i.getStateDefinition())
  }
}

class ContractPart {
  constructor(node) {
    this.node = node
  }

  isStateDefinition() {
    return this.node.type === 'StateVariableDeclaration'
  }

  getStateDefinition() {
    return new StateDefinition(this._firstChild())
  }

  _firstChild() {
    return _.first(this.node.variables)
  }
}

class StateDefinition {
  constructor(node) {
    this.node = node
  }

  stateName() {
    return this.node.name
  }
}

class AssignOperator {
  static of(node, inputSrc) {
    if (node.expression.type === 'BinaryOperation') {
      return new AssignOperator(node, inputSrc)
    }
  }

  constructor(node, inputSrc) {
    this.node = node
    this.inputSrc = inputSrc
  }

  modifyOneOf(states) {
    const assigneeText = this._assignee()

    return states.some((curStateName) => assigneeText.includes(curStateName))
  }

  _assignee() {
    return this.inputSrc.slice(
      this.node.expression.left.range[0],
      this.node.expression.left.range[1] + 1
    )
  }
}

module.exports = ReentrancyChecker
