module.exports = [
  {
    description: 'State variable declaration after function',
    code: `
  contract MyContract {
    function foo() public {}

    uint a;
  }
`,
  },
  {
    description: 'Library after contract',
    code: `
  contract MyContract {}

  library MyLibrary {}
`,
  },
  {
    description: 'Interface after library',
    code: `
  library MyLibrary {}

  interface MyInterface {}
`,
  },
  {
    description: 'Use for after state variable',
    code: `
contract MyContract {
  uint public x;
  
  using MyMathLib for uint;
}
`,
  },
  {
    description: 'External pure before external view',
    code: `
contract MyContract {
  function myExternalFunction() external {}
  function myExternalPureFunction() external pure {}
  function myExternalViewFunction() external view {}
}
`,
  },
  {
    description: 'Public pure before public view',
    code: `
contract MyContract {
  function myPublicFunction() public {}
  function myPublicPureFunction() public pure {}
  function myPublicViewFunction() public view {}
}
`,
  },
  {
    description: 'Internal pure before internal view',
    code: `
contract MyContract {
  function myInternalFunction() internal {}
  function myInternalPureFunction() internal pure {}
  function myInternalViewFunction() internal view {}
}
`,
  },
  {
    description: 'Private pure before private view',
    code: `
contract MyContract {
  function myPrivateFunction() private {}
  function myPrivatePureFunction() private pure {}
  function myPrivateViewFunction() private view {}
}
`,
  },
]
