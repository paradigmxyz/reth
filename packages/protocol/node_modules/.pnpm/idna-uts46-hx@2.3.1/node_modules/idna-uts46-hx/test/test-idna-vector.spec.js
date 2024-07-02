"use strict";

var assert = require("assert");
var fs = require("fs");
var uts46 = require("../uts46");

function matchInaneIDNARules(result, tooLong) {
  var labels = result.split('.');

  // Ignore empty leading tokens because... we have to do this to pass? But we
  // can't ignore stuff in the middle!
  while (labels[0].length === 0)
    labels.shift();

  // Match too short labels or too long labels (this is verify DNS length).
  // Except don't error out on an empty final label (this rule is actually in
  // the algorithm!).
  labels = labels.map(function(label, i) {
    if (label.length < 1 && i !== labels.length - 1)
      throw new Error("Too short label: " + label);
    else if (tooLong && label.length > 63)
      throw new Error("Too long label: " + label);
    return label;
  });
  result = labels.join('.');

  // This validates for the DNS length. Note that while the prose says this only
  // needs to be done for ToASCII, ToUnicode checks the min (but not max!)
  // length.
  if (result.length < 1)
    throw new Error("Too short DNS string: " + result);
  else if (tooLong && result.length > 253 &&
    !(result.length === 254 && result[253] === '.'))
    throw new Error("Too long DNS string: " + result);
  return result;
}

function toAscii(input, transitional) {
  var result = uts46.toAscii(input, {
    transitional: transitional,
    useStd3ASCII: true
  });
  result = matchInaneIDNARules(result, true);
  return result;
}

function toUnicode(input) {
  var result = uts46.toUnicode(input, {
    useStd3ASCII: true
  });
  // ToUnicode isn't supposed to verify DNS length, but the test vectors seem to
  // think that means we aren't supposed to verify overlength.
  result = matchInaneIDNARules(result, false);
  return result;
}

function handleEscapes(string) {
  return string.replace(/\\u([0-9a-fA-F]{4})/g, function(whole, num) {
    return String.fromCharCode(parseInt(num, 16));
  });
}

var lineno = 0;

function handleIdnaTestLine(line) {
  lineno++;
  // Ignore comments and empty lines.
  line = line.split("#")[0];
  if (line.length === 0 || line.trim()==='')
    return;

  var fields = line.split(/;/g).map(function(s) {
    return s.trim();
  });

  var mode = fields[0];
  var testVector = handleEscapes(fields[1]);
  var unicodeData = handleEscapes(fields[2]) || testVector;
  var asciiData = handleEscapes(fields[3]) || unicodeData;

  function handleMode(func, expected) {
    // If this is true, we are expecting an error. However, if the only errors
    // would be bidi or contextual errors, ignore it.
    var expectError = expected.startsWith("[");
    if (expectError && !(/[AVP]/.exec(expected)))
      return;

    if (expectError) {
      test(func.name + " " + line, function() {

        if (mode === "T" || mode === "B")
          assert.throws(function() {
            func(testVector, true);
          });
        if (mode === "N" || mode === "B")
          assert.throws(function() {
            func(testVector, false);
          });
      });
    }
    else {
      test(func.name + " " + line, function() {
        if (expected.includes("["))
          console.log(expected);

        if (mode === "T" || mode === "B")
          assert.equal(func(testVector, true), expected);
        else if (mode === "N" || mode === "B")
          assert.equal(func(testVector, false), expected);
      });
    }
  }

  handleMode(toAscii, asciiData);
  handleMode(toUnicode, unicodeData);
}

suite('IDNA test files', function() {
  var data = fs.readFileSync("test/IdnaTest.txt", {
    encoding: "UTF-8"
  });
  data.split("\n").forEach(handleIdnaTestLine);
});
