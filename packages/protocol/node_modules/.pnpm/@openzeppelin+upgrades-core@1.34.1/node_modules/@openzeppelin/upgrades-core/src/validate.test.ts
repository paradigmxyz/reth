import _test, { TestFn } from 'ava';
import { artifacts } from 'hardhat';

import {
  validate,
  getStorageLayout,
  getContractVersion,
  assertUpgradeSafe,
  ValidationOptions,
  RunValidation,
  ValidationErrors,
} from './validate';
import { solcInputOutputDecoder } from './src-decoder';

interface Context {
  validation: RunValidation;
}

const test = _test as TestFn<Context>;

test.before(async t => {
  const contracts = [
    'contracts/test/Validations.sol:HasEmptyConstructor',
    'contracts/test/ValidationsNatspec.sol:HasNonEmptyConstructorNatspec1',
    'contracts/test/Proxiable.sol:ChildOfProxiable',
    'contracts/test/ValidationsUDVT.sol:ValidationsUDVT',
    'contracts/test/ValidationsFunctionPointers.sol:InternalFunctionPointer',
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

function testValid(name: string, kind: ValidationOptions['kind'], valid: boolean, numExpectedErrors?: number) {
  testOverride(name, kind, {}, valid, numExpectedErrors);
}

function testOverride(
  name: string,
  kind: ValidationOptions['kind'],
  opts: ValidationOptions,
  valid: boolean,
  numExpectedErrors?: number,
) {
  if (numExpectedErrors !== undefined && numExpectedErrors > 0 && valid) {
    throw new Error('Cannot expect errors for a valid contract');
  }

  const optKeys = Object.keys(opts);
  const describeOpts = optKeys.length > 0 ? '(' + optKeys.join(', ') + ')' : '';
  const testName = [valid ? 'accepts' : 'rejects', kind, name, describeOpts].join(' ');
  test(testName, t => {
    const version = getContractVersion(t.context.validation, name);
    const assertUpgSafe = () => assertUpgradeSafe([t.context.validation], version, { kind, ...opts });
    if (valid) {
      t.notThrows(assertUpgSafe);
    } else {
      const error = t.throws(assertUpgSafe) as ValidationErrors;
      if (numExpectedErrors !== undefined) {
        t.is(error.errors.length, numExpectedErrors);
      }
    }
  });
}

testValid('HasEmptyConstructor', 'transparent', true);
testValid('HasConstantStateVariableAssignment', 'transparent', true);
testValid('HasStateVariable', 'transparent', true);
testValid('UsesImplicitSafeInternalLibrary', 'transparent', true);
testValid('UsesExplicitSafeInternalLibrary', 'transparent', true);

testValid('HasNonEmptyConstructor', 'transparent', false);
testValid('ParentHasNonEmptyConstructor', 'transparent', false);
testValid('AncestorHasNonEmptyConstructor', 'transparent', false);
testValid('HasStateVariableAssignment', 'transparent', false);
testValid('HasImmutableStateVariable', 'transparent', false);
testValid('HasSelfDestruct', 'transparent', false);
testValid('HasDelegateCall', 'transparent', false);
testValid('ImportedParentHasStateVariableAssignment', 'transparent', false);
testValid('UsesImplicitUnsafeInternalLibrary', 'transparent', false);
testValid('UsesExplicitUnsafeInternalLibrary', 'transparent', false);
testValid('UsesImplicitUnsafeExternalLibrary', 'transparent', false);
testValid('UsesExplicitUnsafeExternalLibrary', 'transparent', false);

// Linked external libraries are not yet supported
// see: https://github.com/OpenZeppelin/openzeppelin-upgrades/issues/52
testValid('UsesImplicitSafeExternalLibrary', 'transparent', false);
testValid('UsesExplicitSafeExternalLibrary', 'transparent', false);

test('inherited storage', t => {
  const version = getContractVersion(t.context.validation, 'StorageInheritChild');
  const layout = getStorageLayout([t.context.validation], version);
  t.is(layout.storage.length, 8);
  for (let i = 0; i < layout.storage.length; i++) {
    t.is(layout.storage[i].label, `v${i}`);
    t.truthy(layout.types[layout.storage[i].type]);
  }
});

testOverride('UsesImplicitSafeExternalLibrary', 'transparent', { unsafeAllowLinkedLibraries: true }, true);
testOverride('UsesExplicitSafeExternalLibrary', 'transparent', { unsafeAllowLinkedLibraries: true }, true);
testOverride('UsesImplicitSafeExternalLibrary', 'transparent', { unsafeAllow: ['external-library-linking'] }, true);
testOverride('UsesExplicitSafeExternalLibrary', 'transparent', { unsafeAllow: ['external-library-linking'] }, true);

testValid('HasEmptyConstructor', 'uups', false);
testValid('HasInternalUpgradeToFunction', 'uups', false);
testValid('HasUpgradeToFunction', 'uups', true);
testValid('HasInternalUpgradeToAndCallFunction', 'uups', false);
testValid('HasUpgradeToAndCallFunction', 'uups', true);
testValid('ParentHasUpgradeToFunction', 'uups', true);
testValid('ParentHasUpgradeToAndCallFunction', 'uups', true);
testValid('ChildOfProxiable', 'uups', true);

testValid('HasNonEmptyConstructorNatspec1', 'transparent', true);
testValid('HasNonEmptyConstructorNatspec2', 'transparent', true);
testValid('HasNonEmptyConstructorNatspec3', 'transparent', true);
testValid('HasNonEmptyConstructorNatspec4', 'transparent', true);
testValid('ParentHasNonEmptyConstructorNatspec1', 'transparent', true);
testValid('ParentHasNonEmptyConstructorNatspec2', 'transparent', true);
testValid('AncestorHasNonEmptyConstructorNatspec1', 'transparent', true);
testValid('AncestorHasNonEmptyConstructorNatspec2', 'transparent', true);
testValid('HasStateVariableAssignmentNatspec1', 'transparent', true);
testValid('HasStateVariableAssignmentNatspec2', 'transparent', true);
testValid('HasStateVariableAssignmentNatspec3', 'transparent', false);
testValid('HasImmutableStateVariableNatspec1', 'transparent', true);
testValid('HasImmutableStateVariableNatspec2', 'transparent', true);
testValid('HasImmutableStateVariableNatspec3', 'transparent', false);
testValid('HasSelfDestructNatspec1', 'transparent', true);
testValid('HasSelfDestructNatspec2', 'transparent', true);
testValid('HasSelfDestructNatspec3', 'transparent', true);
testValid('HasDelegateCallNatspec1', 'transparent', true);
testValid('HasDelegateCallNatspec2', 'transparent', true);
testValid('HasDelegateCallNatspec3', 'transparent', true);
testValid('ImportedParentHasStateVariableAssignmentNatspec1', 'transparent', true);
testValid('ImportedParentHasStateVariableAssignmentNatspec2', 'transparent', true);
testValid('UsesImplicitSafeInternalLibraryNatspec', 'transparent', true);
testValid('UsesImplicitSafeExternalLibraryNatspec', 'transparent', true);
testValid('UsesImplicitUnsafeInternalLibraryNatspec', 'transparent', true);
testValid('UsesImplicitUnsafeExternalLibraryNatspec', 'transparent', true);
testValid('UsesExplicitSafeInternalLibraryNatspec', 'transparent', true);
testValid('UsesExplicitSafeExternalLibraryNatspec', 'transparent', true);
testValid('UsesExplicitUnsafeInternalLibraryNatspec', 'transparent', true);
testValid('UsesExplicitUnsafeExternalLibraryNatspec', 'transparent', true);
testValid('TransitiveLibraryIsUnsafe', 'transparent', false);

testValid('contracts/test/ValidationsSameNameSafe.sol:SameName', 'transparent', true);
testValid('contracts/test/ValidationsSameNameUnsafe.sol:SameName', 'transparent', false);

test('ambiguous name', t => {
  const error = t.throws(() => getContractVersion(t.context.validation, 'SameName'));
  t.is(
    error?.message,
    'Contract SameName is ambiguous. Use one of the following:\n' +
      'contracts/test/ValidationsSameNameSafe.sol:SameName\n' +
      'contracts/test/ValidationsSameNameUnsafe.sol:SameName',
  );
});

testValid('NamespacedExternalFunctionPointer', 'transparent', true);
testValid('NamespacedInternalFunctionPointer', 'transparent', false);
testValid('NamespacedInternalFunctionPointerUsed', 'transparent', false, 1);
testValid('StructInternalFunctionPointerUsed', 'transparent', false, 1);
testValid('NonNamespacedInternalFunctionPointer', 'transparent', true);
testValid('NamespacedImpliedInternalFunctionPointer', 'transparent', false);
testOverride(
  'NamespacedImpliedInternalFunctionPointer',
  'transparent',
  { unsafeAllow: ['internal-function-storage'] },
  true,
);

testValid('UsesStandaloneStructInternalFn', 'transparent', false);
testValid('NamespacedUsesStandaloneStructInternalFn', 'transparent', false);
testValid('RecursiveStructInternalFn', 'transparent', false);
testValid('MappingRecursiveStructInternalFn', 'transparent', false);
testValid('ArrayRecursiveStructInternalFn', 'transparent', false);
testValid('SelfRecursiveMappingStructInternalFn', 'transparent', false);
testValid('SelfRecursiveArrayStructInternalFn', 'transparent', false);

testValid('ExternalFunctionPointer', 'transparent', true);
testValid('InternalFunctionPointer', 'transparent', false);
testValid('ImpliedInternalFunctionPointer', 'transparent', false);
testOverride('ImpliedInternalFunctionPointer', 'transparent', { unsafeAllow: ['internal-function-storage'] }, true);

testValid('FunctionWithInternalFunctionPointer', 'transparent', true);
