(function(root, factory) {
  /* istanbul ignore next */
  if (typeof define === 'function' && define.amd) {
    define(['punycode', './idna-map'], function(punycode, idna_map) {
      return factory(punycode, idna_map);
    });
  }
  else if (typeof exports === 'object') {
    module.exports = factory(require('punycode'), require('./idna-map'));
  }
  else {
    root.uts46 = factory(root.punycode, root.idna_map);
  }
}(this, function(punycode, idna_map) {

  function mapLabel(label, useStd3ASCII, transitional) {
    var mapped = [];
    var chars = punycode.ucs2.decode(label);
    for (var i = 0; i < chars.length; i++) {
      var cp = chars[i];
      var ch = punycode.ucs2.encode([chars[i]]);
      var composite = idna_map.mapChar(cp);
      var flags = (composite >> 23);
      var kind = (composite >> 21) & 3;
      var index = (composite >> 5) & 0xffff;
      var length = composite & 0x1f;
      var value = idna_map.mapStr.substr(index, length);
      if (kind === 0 || (useStd3ASCII && (flags & 1))) {
        throw new Error("Illegal char " + ch);
      }
      else if (kind === 1) {
        mapped.push(value);
      }
      else if (kind === 2) {
        mapped.push(transitional ? value : ch);
      }
      /* istanbul ignore next */
      else if (kind === 3) {
        mapped.push(ch);
      }
    }

    var newLabel = mapped.join("").normalize("NFC");
    return newLabel;
  }

  function process(domain, transitional, useStd3ASCII) {
    /* istanbul ignore if */
    if (useStd3ASCII === undefined)
      useStd3ASCII = false;
    var mappedIDNA = mapLabel(domain, useStd3ASCII, transitional);

    // Step 3. Break
    var labels = mappedIDNA.split(".");

    // Step 4. Convert/Validate
    labels = labels.map(function(label) {
      if (label.startsWith("xn--")) {
        label = punycode.decode(label.substring(4));
        validateLabel(label, useStd3ASCII, false);
      }
      else {
        validateLabel(label, useStd3ASCII, transitional);
      }
      return label;
    });

    return labels.join(".");
  }

  function validateLabel(label, useStd3ASCII, transitional) {
    // 2. The label must not contain a U+002D HYPHEN-MINUS character in both the
    // third position and fourth positions.
    if (label[2] === '-' && label[3] === '-')
      throw new Error("Failed to validate " + label);

    // 3. The label must neither begin nor end with a U+002D HYPHEN-MINUS
    // character.
    if (label.startsWith('-') || label.endsWith('-'))
      throw new Error("Failed to validate " + label);

    // 4. The label must not contain a U+002E ( . ) FULL STOP.
    // this should nerver happen as label is chunked internally by this character
    /* istanbul ignore if */
    if (label.includes('.'))
      throw new Error("Failed to validate " + label);

    if (mapLabel(label, useStd3ASCII, transitional) !== label)
      throw new Error("Failed to validate " + label);

    // 5. The label must not begin with a combining mark, that is:
    // General_Category=Mark.
    var ch = label.codePointAt(0);
    if (idna_map.mapChar(ch) & (0x2 << 23))
      throw new Error("Label contains illegal character: " + ch);
  }

  function toAscii(domain, options) {
    if (options === undefined)
      options = {};
    var transitional = 'transitional' in options ? options.transitional : true;
    var useStd3ASCII = 'useStd3ASCII' in options ? options.useStd3ASCII : false;
    var verifyDnsLength = 'verifyDnsLength' in options ? options.verifyDnsLength : false;
    var labels = process(domain, transitional, useStd3ASCII).split('.');
    var asciiLabels = labels.map(punycode.toASCII);
    var asciiString = asciiLabels.join('.');
    var i;
    if (verifyDnsLength) {
      if (asciiString.length < 1 || asciiString.length > 253) {
        throw new Error("DNS name has wrong length: " + asciiString);
      }
      for (i = 0; i < asciiLabels.length; i++) {//for .. of replacement
        var label = asciiLabels[i];
        if (label.length < 1 || label.length > 63)
          throw new Error("DNS label has wrong length: " + label);
      }
    }
    return asciiString;
  }

  function toUnicode(domain, options) {
    if (options === undefined)
      options = {};
    var useStd3ASCII = 'useStd3ASCII' in options ? options.useStd3ASCII : false;
    return process(domain, false, useStd3ASCII);
  }

  return {
    toUnicode: toUnicode,
    toAscii: toAscii,
  };
}));
