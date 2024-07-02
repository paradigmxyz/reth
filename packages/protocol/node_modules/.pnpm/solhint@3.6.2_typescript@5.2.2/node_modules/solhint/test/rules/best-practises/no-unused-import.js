const linter = require('../../../lib/index')
const {
  assertNoErrors,
  assertErrorMessage,
  assertErrorCount,
  assertWarnsCount,
} = require('../../common/asserts')

describe('Linter - no-unused-import', () => {
  it('should raise when imported name is not used', () => {
    const code = `import {A} from './A.sol';`

    const report = linter.processStr(code, {
      rules: { 'no-unused-import': 'error' },
    })
    assertErrorCount(report, 1)
    assertErrorMessage(report, 'imported name A is not used')
  })

  it('should raise when name created in import "path" as name is not used', () => {
    const code = `import './A.sol' as A;`

    const report = linter.processStr(code, {
      rules: { 'no-unused-import': 'error' },
    })
    assertErrorCount(report, 1)
    assertErrorMessage(report, 'imported name A is not used')
  })

  it('should not crash on, but also not recognize, malformed inheritdoc statements', () => {
    const code = `
    import {A} from './A.sol';
    // inheritdoc A
    // @inheritdoc A
    /// inheritdoc A
    /// @ inherit A
    /// @ inheritdoc A
    /// @inheritdoc somethingelse
    /// @inheritdoc
    /// @inheritdoc 
    `

    const report = linter.processStr(code, {
      rules: { 'no-unused-import': 'error' },
    })
    assertErrorCount(report, 1)
    assertErrorMessage(report, 'imported name A is not used')
  })

  it('should raise error when using solhint:recommended', () => {
    const code = `pragma solidity ^0.8.0; import {A} from "./A.sol";`

    const report = linter.processStr(code, {
      extends: 'solhint:recommended',
    })
    assertWarnsCount(report, 1)
    assertErrorMessage(report, 'imported name A is not used')
  })

  it('should report correct name when unused import is aliased', () => {
    const code = `import {A as B} from './A.sol';`

    const report = linter.processStr(code, {
      rules: { 'no-unused-import': 'error' },
    })
    assertErrorCount(report, 1)
    assertErrorMessage(report, 'imported name B is not used')
  })

  it('should raise when some of the imported names are not used', () => {
    const code = `import {A, B} from './A.sol'; contract C is A {}`

    const report = linter.processStr(code, {
      rules: { 'no-unused-import': 'error' },
    })
    assertErrorCount(report, 1)
    assertErrorMessage(report, 'imported name B is not used')
  })

  it('should not raise when contract name is used as a type for a memory variable', () => {
    const code = `
    import {ERC20} from './ERC20.sol';
    contract A {
      function fun () public {
        ERC20 funToken = address(0);
      }
    }`

    const report = linter.processStr(code, {
      rules: { 'no-unused-import': 'error' },
    })
    assertNoErrors(report)
  })
  ;[
    {
      description: 'a field of an imported name is used',
      code: `import {Vm} from './Vm.sol';
            contract A {
              function fun () public {
                Vm.cheat('code');
              }
            }`,
    },
    {
      description: 'imported name is used in new statement',
      code: `import {OtherContract} from './Contract.sol';
            contract Factory {
              function deploy () public returns (address){
                return new OtherContract();
              }
            }`,
    },
    {
      description: 'imported name is used in array definition',
      code: `import {B} from './Contract.sol';
            contract A {
              function fun () public {
                B[] memory someArray;
              }
            }`,
    },
    {
      description: 'member of imported name is used in array definition',
      code: `import {B} from './Contract.sol';
            contract A {
              function fun () public {
                B.field[] memory someArray;
              }
            }`,
    },
    {
      description: 'imported name is used in type cast',
      code: `import {ImportedType} from './Contract.sol';
            contract A {
              function fun () public {
                ImportedType(address(0));
              }
            }`,
    },
    {
      description: 'imported name is used as a custom error',
      code: `import {SpecialError} from './Contract.sol';
            contract A {
              function fun () public {
                revert SpecialError();
              }
            }`,
    },
    {
      description: 'contract name is used for inheritance',
      code: `import {A} from './A.sol'; contract B is A {}`,
    },
    {
      description: 'aliased contract name is used',
      code: `import {A as B} from './A.sol'; contract C is B {}`,
    },
    {
      description: 'libary name is used in a using ... for statement',
      code: `import {A} from './A.sol'; contract B { using A for uint256; }`,
    },
    {
      description: 'contract name is used in a state variable declaration',
      code: ` import {A} from './A.sol'; contract B { A public statevar; }`,
    },
    {
      description: 'imported subtype is used in a state variable declaration',
      code: `import {A} from './A.sol'; contract B { A.thing public statevar; }`,
    },
    {
      description: 'imported type is used in a function parameter declaration',
      code: `import {A} from './A.sol'; contract B { function (A.thing statevar) public {} }`,
    },
    {
      description: 'imported function is attached to a type',
      code: `import {add} from './A.sol'; type Int is int; using {add} for Int global;`,
    },
    {
      description: 'imported function is used as a user-defined operator',
      code: `import {add} from './A.sol'; type Int is int; using {add as +} for Int global;`,
    },
    {
      description: 'field of imported name is attached to a type',
      code: `
          import {Int, Math} from "./A.sol"; using {Math.add} for Int;
          contract C {
              Int public foo;
          }
      `,
    },
    {
      description: 'Name is used in an override',
      code: `
        pragma solidity >=0.8.19;

        import { ERC721 } from "@openzeppelin/contracts/token/ERC721/ERC721.sol";
        import { IERC721Metadata } from "@openzeppelin/contracts/token/ERC721/extensions/IERC721Metadata.sol";

        contract MyContract {
            function tokenURI(uint256 streamId) public view override(IERC721Metadata, ERC721) returns (string memory uri) {
                uri = "example.com";
            }
        }
      `,
    },
    {
      description: 'Type is used in a constructor initializator',
      code: `
        pragma solidity >=0.8.19;

        import { ERC721 } from "@openzeppelin/contracts/token/ERC721/ERC721.sol";

        contract MyContract {
            constructor() ERC721("Sablier V2 Lockup Dynamic NFT", "SAB-V2-LOCKUP-DYN") { }
        }
      `,
    },
    {
      description: 'Import is used as value in a function parameter',
      code: `
        pragma solidity >=0.8.19 <0.9.0;

        import { UD60x18, ZERO } from "@prb/math/UD60x18.sol";

        contract SetProtocolFee_Integration_Fuzz_Test {
            function testFuzz_SetProtocolFee(UD60x18 newProtocolFee) external {
                newProtocolFee = _bound(newProtocolFee, 1, MAX_FEE);
                vm.expectEmit({ emitter: address(comptroller) });
                emit SetProtocolFee({ admin: users.admin, asset: dai, oldProtocolFee: ZERO, newProtocolFee: newProtocolFee });
            }
        }
      `,
    },
    {
      description: 'Import is used as value in a binary expression',
      code: `
        import { ZERO } from "@prb/math/UD60x18.sol";

        contract Foo {
            function returnFifteen() public returns (uint){
              return ZERO + 15;
            }
        }
      `,
    },
    {
      description: 'Import is used as value in an assignment',
      code: `
        import { ZERO } from "@prb/math/UD60x18.sol";

        contract Foo {
            uint256 public howMuch;
            constructor () {
              howMuch = ZERO;
            }
        }
      `,
    },
    {
      description: 'Import is used in the left side of a using for statement',
      code: `
        pragma solidity >=0.8.19;

        import {EnumerableSetUD60x18, EnumerableSet} from "./libraries/EnumerableSetUD60x18.sol";

        contract SolhintTest {
            using EnumerableSetUD60x18 for EnumerableSet.Bytes32Set;
        }
      `,
    },
    {
      description: 'Import is used as the type value of a mapping',
      code: `
        pragma solidity >=0.8.19;

        import {UD60x18} from "@prb/math/UD60x18.sol";

        contract SolhintTest {
            mapping(address user => UD60x18 amount) internal balance;
        }
      `,
    },
    {
      description: 'Import is used as the type value of a nested mapping',
      code: `
        pragma solidity >=0.8.19;
        import {SD59x18} from "@prb/math/SD59x18.sol";

        contract SolhintTest {
            mapping(address user => mapping(address relayer => SD59x18 amount)) internal balance;
        }
      `,
    },
    {
      description: 'Import is used in /// @inheritdoc',
      code: `
        import { IPRBProxyPlugin } from "@prb/proxy/interfaces/IPRBProxyPlugin.sol";

        contract SablierV2ProxyPlugin {
          /// @inheritdoc IPRBProxyPlugin
          function getMethods() external pure returns (bytes4[] memory methods) {
              methods = new bytes4[](1);
              methods[0] = this.onStreamCanceled.selector;
          }
        }
      `,
    },
    {
      description: 'Import is used in /** @inheritdoc',
      code: `
        import { IPRBProxyPlugin } from "@prb/proxy/interfaces/IPRBProxyPlugin.sol";

        contract SablierV2ProxyPlugin {
          /** @inheritdoc IPRBProxyPlugin */
          function getMethods() external pure returns (bytes4[] memory methods) {
              methods = new bytes4[](1);
              methods[0] = this.onStreamCanceled.selector;
          }
        }
      `,
    },
  ].forEach(({ description, code }) => {
    it(`should not raise when ${description}`, () => {
      const report = linter.processStr(code, {
        rules: { 'no-unused-import': 'error' },
      })
      assertNoErrors(report)
    })
  })
})
