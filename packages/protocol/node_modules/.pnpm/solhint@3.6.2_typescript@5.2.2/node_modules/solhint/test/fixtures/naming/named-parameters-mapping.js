const NAMED_MAPPING_REGULAR = 'mapping(string ownerName => address owner) public ownerAddresses;'

const NAMED_MAPPING_NESTED =
  'mapping(address owner => mapping(address token => uint256 balance)) public tokenBalances;'

const NO_NAMED_MAPPING_REGULAR = [
  {
    code: 'mapping(string => address owner) public ownerAddresses;',
    error_mainKey: true,
    error_value: false,
    mapping_name: 'ownerAddresses',
  },
  {
    code: 'mapping(string ownerName => address) public ownerAddresses;',
    error_mainKey: false,
    error_value: true,
    mapping_name: 'ownerAddresses',
  },
  {
    code: 'mapping(string => address) public ownerAddresses;',
    error_mainKey: true,
    error_value: true,
    mapping_name: 'ownerAddresses',
  },
]

const NO_NAMED_MAPPING_NESTED = [
  {
    code: 'mapping(address => mapping(address => uint256)) public tokenBalances;',
    error_mainKey: true,
    error_value: false,
    mapping_name: 'tokenBalances',
  },
  {
    code: 'mapping(address => mapping(address token => uint256)) public tokenBalances;',
    error_mainKey: true,
    error_value: false,
    mapping_name: 'tokenBalances',
  },
  {
    code: 'mapping(address => mapping(address => uint256 balance)) public tokenBalances;',
    error_mainKey: true,
    error_value: false,
    mapping_name: 'tokenBalances',
  },
  {
    code: 'mapping(address => mapping(address token => uint256 balance)) public tokenBalances;',
    error_mainKey: true,
    error_value: false,
    mapping_name: 'tokenBalances',
  },
]

const OTHER_WRONG_DECLARATIONS = [
  {
    code: `
    struct ThisIsStruct {
      uint256 A;
      uint256 B;
    }
    mapping(address => ThisIsStruct) public ownerStuff;`,
    error_mainKey: true,
    error_value: true,
    mapping_name: 'ownerStuff',
  },
  {
    code: `
    struct ThisIsStruct {
      uint256 A;
      uint256 B;
    }
    mapping(address owner => ThisIsStruct) public ownerStuff;`,
    error_mainKey: false,
    error_value: true,
    mapping_name: 'ownerStuff',
  },
  {
    code: `
    struct ThisIsStruct {
      uint256 A;
      uint256 B;
    }
    mapping(address => mapping(address token => ThisIsStruct)) public ownerStuffPerToken;`,
    error_mainKey: true,
    error_value: false,
    mapping_name: 'ownerStuffPerToken',
  },
  {
    code: `
    uint256 public A;
    struct ThisIsStruct {
      uint256 A;
      uint256 B;
    }
    mapping(address => mapping(address => ThisIsStruct)) public ownerStuffPerToken;`,
    error_mainKey: true,
    error_value: false,
    mapping_name: 'ownerStuffPerToken',
  },
]

const OTHER_OK_DECLARATIONS = [
  {
    code: `
    struct ThisIsStruct {
      uint256 A;
      uint256 B;
    }
    mapping(address owner => ThisIsStruct structContent) public ownerStuff;
    uint256 public A;`,
    error_mainKey: false,
    error_value: false,
    mapping_name: 'ownerStuff',
  },
  {
    code: 'uint256 public A;',
    error_mainKey: false,
    error_value: false,
  },
  {
    code: 'uint256 constant public A = 100000;',
    error_mainKey: false,
    error_value: false,
  },
  {
    code: `
    struct ThisIsStruct {
      uint256 A;
      uint256 B;
    }`,
    error_mainKey: false,
    error_value: false,
  },
  {
    code: `
    struct ThisIsStruct {
      uint256 A;
      uint256 B;
    }
    mapping(address owner => mapping(address token => ThisIsStruct structContent)) public ownerStuffPerToken;`,
    error_mainKey: false,
    error_value: false,
    mapping_name: 'ownerStuffPerToken',
  },
  {
    code: `
    struct ThisIsStruct {
      uint256 A;
      uint256 B;
    }
    mapping(address owner => mapping(address => ThisIsStruct structContent)) public ownerStuffPerToken;`,
    error_mainKey: false,
    error_value: false,
    mapping_name: 'ownerStuffPerToken',
  },
  {
    code: `
    struct ThisIsStruct {
      uint256 A;
      uint256 B;
    }
    mapping(address owner => mapping(address => ThisIsStruct)) public ownerStuffPerToken;`,
    error_mainKey: false,
    error_value: false,
    mapping_name: 'ownerStuffPerToken',
  },
]

module.exports = {
  NAMED_MAPPING_REGULAR,
  NO_NAMED_MAPPING_REGULAR,
  NAMED_MAPPING_NESTED,
  NO_NAMED_MAPPING_NESTED,
  OTHER_OK_DECLARATIONS,
  OTHER_WRONG_DECLARATIONS,
}
