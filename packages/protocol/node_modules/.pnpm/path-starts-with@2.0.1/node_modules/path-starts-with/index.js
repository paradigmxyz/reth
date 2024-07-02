'use strict';

const toPosixSlashes = str => str.replace(/\\/g, '/');
const isPathSeparator = code => code === 47 || code === 92;
const isDot = code => code === 46;

const removeDotSlash = str => {
  if (isDot(str.charCodeAt(0)) && isPathSeparator(str.charCodeAt(1))) {
    return str.slice(2);
  }
  return str;
};

const countLeadingSlashes = str => {
  let i = 0;
  for (; i < str.length; i++) {
    if (!isPathSeparator(str.charCodeAt(i))) {
      break;
    }
  }
  return i;
};

const startsWith = (filepath, substr, options) => {
  if (typeof filepath !== 'string') {
    throw new TypeError('expected filepath to be a string');
  }
  if (typeof substr !== 'string') {
    throw new TypeError('expected substring to be a string');
  }

  filepath = removeDotSlash(filepath);
  substr = removeDotSlash(substr);

  if (filepath === substr) return true;

  if (substr === '' || filepath === '') {
    return false;
  }

  if (options && options.nocase !== false) {
    substr = substr.toLowerCase();
    filepath = filepath.toLowerCase();
  }

  // return true if the lowercased strings are an exact match
  if (filepath === substr) return true;

  if (substr.charAt(0) === '!' && (!options || !options.nonegate)) {
    return !startsWith(filepath, substr.slice(1), options);
  }

  // normalize slashes in substring and filepath
  const fp = toPosixSlashes(filepath);
  const str = toPosixSlashes(substr);

  // now that slashes are normalized, check for an exact match again
  if (fp === str) return true;
  if (!fp.startsWith(str)) return false;

  // if partialMatch is enabled, we have a match
  if (options && options.partialMatch === true) {
    return true;
  }

  const substrSlashesLen = countLeadingSlashes(substr);
  const filepathSlashesLen = countLeadingSlashes(filepath);

  // if substring consists of only slashes, the
  // filepath must begin with the same number of slashes
  if (substrSlashesLen === substr.length) {
    return filepathSlashesLen === substrSlashesLen;
  }

  // handle "C:/foo" matching "C:/"
  if (str.endsWith('/') && /^[A-Z]:\//.test(fp)) {
    return true;
  }

  return fp[str.length] === '/';
};

module.exports = startsWith;
