const fs = require('fs');
const path = require('path');
const shell = require('shelljs');
const assert = require('assert');

function pathExists(path) { return shell.test('-e', path); }

function lineCoverage(expected=[]){
  let summary = JSON.parse(fs.readFileSync('coverage/coverage-summary.json'));

  expected.forEach((item, idx) => {
    assert(
      summary[item.file].lines.pct === item.pct,

      `For condition ${idx} - expected ${item.pct} %,` +
      `saw - ${summary[item.file].lines.pct} %`
    )
  });
}

function statementCoverage(expected=[], summaryKey='pct'){
  let summary = JSON.parse(fs.readFileSync('coverage/coverage-summary.json'));

  expected.forEach((item, idx) => {
    assert(
      summary[item.file].statements[summaryKey] === item[summaryKey],

      `For condition ${idx} - expected ${item[summaryKey]} %,` +
      `saw - ${summary[item.file].statements[summaryKey]} %`
    )
  });
}

function functionCoverage(expected=[], summaryKey='pct'){
  let summary = JSON.parse(fs.readFileSync('coverage/coverage-summary.json'));

  expected.forEach((item, idx) => {
    assert(
      summary[item.file].functions[summaryKey] === item[summaryKey],

      `For condition ${idx} - expected ${item[summaryKey]} %,` +
      `saw - ${summary[item.file].functions[summaryKey]} %`
    )
  });
}

function branchCoverage(expected=[], summaryKey='pct'){
  let summary = JSON.parse(fs.readFileSync('coverage/coverage-summary.json'));

  expected.forEach((item, idx) => {
    assert(
      summary[item.file].branches.pct === item[summaryKey],

      `For condition ${idx} - expected ${item[summaryKey]} %,` +
      `saw - ${summary[item.file].branches[summaryKey]} %`
    )
  });
}

function coverageMissing(expected=[]){
  let summary = JSON.parse(fs.readFileSync('coverage/coverage-summary.json'));
  expected.forEach(item => assert(summary[item.file] === undefined))
}

function cleanInitialState(){
  assert(pathExists('./coverage') === false, 'should start without: coverage');
  assert(pathExists('./coverage.json') === false, 'should start without: coverage.json');
}

function coverageGenerated(config){
  const workingDir = config.working_directory || config.paths.root;
  const jsonPath = path.join(workingDir, "coverage.json");

  assert(pathExists('./coverage') === true, 'should gen coverage folder');
  assert(pathExists(jsonPath) === true, 'should gen coverage.json');
}

function coverageNotGenerated(config){
  const workingDir = config.working_directory || config.paths.root;
  const jsonPath = path.join(workingDir, "coverage.json");

  assert(pathExists('./coverage') !== true, 'should NOT gen coverage folder');
  assert(pathExists(jsonPath) !== true, 'should NOT gen coverage.json');
}

module.exports = {
  pathExists: pathExists,
  lineCoverage: lineCoverage,
  statementCoverage: statementCoverage,
  functionCoverage: functionCoverage,
  branchCoverage: branchCoverage,
  coverageMissing: coverageMissing,
  cleanInitialState: cleanInitialState,
  coverageGenerated: coverageGenerated,
  coverageNotGenerated: coverageNotGenerated,
}
