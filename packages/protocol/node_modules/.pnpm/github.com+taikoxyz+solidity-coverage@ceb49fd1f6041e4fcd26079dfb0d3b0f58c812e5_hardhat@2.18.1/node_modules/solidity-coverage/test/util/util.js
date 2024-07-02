/**
 * Setup and reporting helpers for the suites which test instrumentation
 * and coverage correctness. (Integration test helpers are elsewhere)
 */
const fs = require('fs');
const path = require('path');
const solc = require('solc');
const TruffleContract = require('@truffle/contract');

const Instrumenter = require('./../../lib/instrumenter');
const DataCollector = require('./../../lib/collector')

// ====================
// Paths / Files
// ====================
const filePath = path.resolve('./test.sol');
const pathPrefix = './';

// ====================
// Contract deployments
// ====================
function getABI(solcOutput, testFile="test.sol", testName="Test"){
  return solcOutput.contracts[testFile][testName].abi;
}

function getBytecode(solcOutput, testFile="test.sol", testName="Test"){
  return `0x${solcOutput.contracts[testFile][testName].evm.bytecode.object}`;
}

async function getDeployedContractInstance(info, provider){

  const contract = TruffleContract({
    abi: getABI(info.solcOutput),
    bytecode: getBytecode(info.solcOutput)
  })

  contract.setProvider(provider);
  contract.autoGas = false;

  const accounts = await contract.web3.eth.getAccounts();
  contract.defaults({
    gas: 5500000,
    gasPrice: 1,
    from: accounts[0]
  });

  return contract.new();
}

// ============
// Compilation
// ============
function getCode(_path) {
  const pathToSources = `./../sources/solidity/contracts/${_path}`;
  return fs.readFileSync(path.join(__dirname, pathToSources), 'utf8');
};

function compile(source){
  const compilerInput = codeToCompilerInput(source);
  return JSON.parse(solc.compile(compilerInput));
}

function codeToCompilerInput(code) {
  return JSON.stringify({
    language: 'Solidity',
    sources: { 'test.sol': { content: code } },
    settings: { outputSelection: {'*': { '*': [ '*' ] }} }
  });
}

// ===========
// Diff tests
// ===========
function getDiffABIs(sourceName, testFile="test.sol", original="Old", current="New"){
  const contract = getCode(`${sourceName}.sol`)
  const solcOutput = compile(contract)
  return {
    original: {
      contractName: "Test",
      sha: "d8b26d8",
      abi: solcOutput.contracts[testFile][original].abi,
    },
    current: {
      contractName: "Test",
      sha: "e77e29d",
      abi: solcOutput.contracts[testFile][current].abi,
    }
  }
}

// ============================
// Instrumentation Correctness
// ============================
function instrumentAndCompile(sourceName, api={}) {
  const contract = getCode(`${sourceName}.sol`)
  const instrumenter = new Instrumenter(api.config);
  const instrumented = instrumenter.instrument(contract, filePath);

  return {
    contract: contract,
    instrumented: instrumented,
    solcOutput: compile(instrumented.contract),
    data: instrumenter.instrumentationData
  }
}

function report(output=[]) {
  output.forEach(item => {
    if (item.severity === 'error') {
      const errors = JSON.stringify(output, null, ' ');
      throw new Error(`Instrumentation fault: ${errors}`);
    }
  });
}

// =====================
// Coverage Correctness
// =====================
async function bootstrapCoverage(file, api){
  const info = instrumentAndCompile(file, api);
  info.instance = await getDeployedContractInstance(info, api.server.provider);
  api.collector._setInstrumentationData(info.data);
  return info;
}

// =========
// Provider
// =========
function initializeProvider(ganache){
  const collector = new DataCollector();
  const options = { logger: { log: collector.step.bind(collector) }};
  const provider = ganache.provider(options);

  return {
    provider: provider,
    collector: collector
  }
}

module.exports = {
  getCode,
  pathPrefix,
  filePath,
  report,
  instrumentAndCompile,
  bootstrapCoverage,
  initializeProvider,
  getDiffABIs
}
