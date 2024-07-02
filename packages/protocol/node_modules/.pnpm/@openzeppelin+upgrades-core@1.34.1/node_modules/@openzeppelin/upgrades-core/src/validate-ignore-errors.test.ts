import _test, { TestFn } from 'ava';
import { artifacts } from 'hardhat';

import { validate, getContractVersion, assertUpgradeSafe, ValidationOptions, RunValidation } from './validate';
import { solcInputOutputDecoder } from './src-decoder';

interface Context {
  validation: RunValidation;
}

const test = _test as TestFn<Context>;

test.before(async t => {
  const contracts = [
    'contracts/test/ignore-errors/SafeContract.sol:SafeContract',
    'contracts/test/ignore-errors/SafeContractWithFreeFunctionCall.sol:SafeContractWithFreeFunctionCall',
    'contracts/test/ignore-errors/SafeContractWithLibraryCall.sol:SafeContractWithLibraryCall',
    'contracts/test/ignore-errors/SafeContractWithLibraryImport.sol:SafeContractWithLibraryImport',
    'contracts/test/ignore-errors/SafeContractWithLibraryUsingFor.sol:SafeContractWithLibraryUsingFor',
    'contracts/test/ignore-errors/SafeContractWithTransitiveLibraryCall.sol:SafeContractWithTransitiveLibraryCall',
    'contracts/test/ignore-errors/SafeContractWithParentCall.sol:SafeContractWithParentCall',
    'contracts/test/ignore-errors/RiskyParentContract.sol:RiskyParentContract',
    'contracts/test/ignore-errors/UnsafeContract.sol:UnsafeContract',
    'contracts/test/ignore-errors/UnsafeContractWithFreeFunctionCall.sol:UnsafeContractWithFreeFunctionCall',
    'contracts/test/ignore-errors/UnsafeContractWithLibraryCall.sol:UnsafeContractWithLibraryCall',
    'contracts/test/ignore-errors/UnsafeContractWithLibraryUsingFor.sol:UnsafeContractWithLibraryUsingFor',
    'contracts/test/ignore-errors/UnsafeContractWithTransitiveLibraryCall.sol:UnsafeContractWithTransitiveLibraryCall',
    'contracts/test/ignore-errors/UnsafeContractWithParentCall.sol:UnsafeContractWithParentCall',
    'contracts/test/ignore-errors/UnsafeContractWithInheritedParent.sol:UnsafeContractWithInheritedParent',
    'contracts/test/ignore-errors/UnsafeContractWithInheritedTransitiveParent.sol:UnsafeContractWithInheritedTransitiveParent',
    'contracts/test/ignore-errors/UnsafeAllow.sol:UnsafeAllow',
    'contracts/test/ignore-errors/AllowReachable.sol:AllowReachable',
    'contracts/test/ignore-errors/UnsafeAllowReachableDifferentOpcode.sol:UnsafeAllowReachableDifferentOpcode',
    'contracts/test/ignore-errors/UnsafeAllowParent.sol:UnsafeAllowParent',
    'contracts/test/ignore-errors/AllowReachableParent.sol:AllowReachableParent',
    'contracts/test/ignore-errors/UnsafeAllowReachableParentDifferentOpcode.sol:UnsafeAllowReachableParentDifferentOpcode',
    'contracts/test/ignore-errors/AllowChildInheritedTransitive.sol:AllowChildInheritedTransitive',
    'contracts/test/ignore-errors/AllowChild.sol:AllowChild',
    'contracts/test/ignore-errors/AllowChildCallTransitive.sol:AllowChildCallTransitive',
    'contracts/test/ignore-errors/AllowChildSelfReachable.sol:AllowChildSelfReachable',
    'contracts/test/ignore-errors/SafeRecursion.sol:SafeRecursion',
    'contracts/test/ignore-errors/UnsafeRecursion.sol:UnsafeRecursion',
    'contracts/test/ignore-errors/Constructors.sol:UnsafeChild1',
    'contracts/test/ignore-errors/Constructors.sol:UnsafeChild2',
    'contracts/test/ignore-errors/Constructors.sol:UnsafeChild3',
    'contracts/test/ignore-errors/Constructors.sol:UnsafeChild4',
    'contracts/test/ignore-errors/Constructors.sol:AllowChild5',
    'contracts/test/ignore-errors/Constructors.sol:AllowChild6',
    'contracts/test/ignore-errors/Constructors.sol:UnsafeAllowChild7',
    'contracts/test/ignore-errors/Constructors.sol:AllowReachableChild8',
    'contracts/test/ignore-errors/Constructors.sol:UnsafeChild9',
    // 'contracts/test/ignore-errors/Modifiers.sol:ModifierNotUsed',
    'contracts/test/ignore-errors/Modifiers.sol:ModifierUsed',
  ];

  t.context.validation = {} as RunValidation;
  for (const contract of contracts) {
    const buildInfo = await artifacts.getBuildInfo(contract);
    if (buildInfo === undefined) {
      throw new Error(`Build info not found for contract ${contract}`);
    }
    const solcOutput = buildInfo.output;
    const solcInput = buildInfo.input;
    const decodeSrc = solcInputOutputDecoder(solcInput, solcOutput);
    Object.assign(t.context.validation, validate(solcOutput, decodeSrc));
  }
});

function testValid(name: string, kind: ValidationOptions['kind'], valid: boolean) {
  testOverride(name, kind, {}, valid);
}

function testOverride(name: string, kind: ValidationOptions['kind'], opts: ValidationOptions, valid: boolean) {
  const optKeys = Object.keys(opts);
  const describeOpts = optKeys.length > 0 ? '(' + optKeys.join(', ') + ')' : '';
  const testName = [valid ? 'accepts' : 'rejects', kind, name, describeOpts].join(' ');
  test(testName, t => {
    const version = getContractVersion(t.context.validation, name);
    const assertUpgSafe = () => assertUpgradeSafe([t.context.validation], version, { kind, ...opts });
    if (valid) {
      t.notThrows(assertUpgSafe);
    } else {
      t.throws(assertUpgSafe);
    }
  });
}

testValid('SafeContract', 'transparent', true);
testValid('SafeContractWithFreeFunctionCall', 'transparent', true);
testValid('SafeContractWithLibraryCall', 'transparent', true);
testValid('SafeContractWithLibraryImport', 'transparent', true);
testValid('SafeContractWithLibraryUsingFor', 'transparent', true);
testValid('SafeContractWithTransitiveLibraryCall', 'transparent', true);
testValid('SafeContractWithParentCall', 'transparent', true);

testValid('RiskyParentContract', 'transparent', false);

testValid('UnsafeContract', 'transparent', false);
testValid('UnsafeContractWithFreeFunctionCall', 'transparent', false);
testValid('UnsafeContractWithLibraryCall', 'transparent', false);
testValid('UnsafeContractWithLibraryUsingFor', 'transparent', false);
testValid('UnsafeContractWithTransitiveLibraryCall', 'transparent', false);
testValid('UnsafeContractWithParentCall', 'transparent', false);
testValid('UnsafeContractWithInheritedParent', 'transparent', false);
testValid('UnsafeContractWithInheritedTransitiveParent', 'transparent', false);

testValid('UnsafeAllow', 'transparent', false);
testValid('AllowReachable', 'transparent', true);
testValid('UnsafeAllowReachableDifferentOpcode', 'transparent', false);

testValid('UnsafeAllowParent', 'transparent', false);
testValid('AllowReachableParent', 'transparent', true);
testValid('UnsafeAllowReachableParentDifferentOpcode', 'transparent', false);

testValid('AllowChildInheritedTransitive', 'transparent', true);
testValid('AllowChild', 'transparent', true);
testValid('AllowChildCallTransitive', 'transparent', true);

testValid('AllowChildSelfReachable', 'transparent', true);

testValid('SafeRecursion', 'transparent', true);
testValid('UnsafeRecursion', 'transparent', false);

testValid('UnsafeChild1', 'transparent', false);
testValid('UnsafeChild2', 'transparent', false);
testValid('UnsafeChild3', 'transparent', false);
testValid('UnsafeChild4', 'transparent', false);
testValid('AllowChild5', 'transparent', true);
testValid('AllowChild6', 'transparent', true);
testValid('UnsafeAllowChild7', 'transparent', false);
testValid('AllowReachableChild8', 'transparent', true);
testValid('UnsafeChild9', 'transparent', false);

// TODO: do not throw an error in this case
// testValid('ModifierNotUsed', 'transparent', true);

testValid('ModifierUsed', 'transparent', false);
