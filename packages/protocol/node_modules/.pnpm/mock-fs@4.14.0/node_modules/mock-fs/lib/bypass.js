const realBinding = process.binding('fs');
let storedBinding;

/**
 * Perform action, bypassing mock FS
 * @example
 * // This file exists on the real FS, not on the mocked FS
 * const filePath = '/path/file.json';
 * const data = mock.bypass(() => fs.readFileSync(filePath, 'utf-8'));
 */
exports = module.exports = function bypass(fn) {
  if (typeof fn !== 'function') {
    throw new Error(`Must provide a function to perform for mock.bypass()`);
  }

  disable();

  let result;
  try {
    result = fn();
  } finally {
    if (result && typeof result.then === 'function') {
      result.then(
        r => {
          enable();
          return r;
        },
        err => {
          enable();
          throw err;
        }
      );
    } else {
      enable();
    }
  }

  return result;
};

/**
 * Temporarily disable Mocked FS
 */
function disable() {
  if (realBinding._mockedBinding) {
    storedBinding = realBinding._mockedBinding;
    delete realBinding._mockedBinding;
  }
}

/**
 * Enables Mocked FS after being disabled by disable()
 */
function enable() {
  if (storedBinding) {
    realBinding._mockedBinding = storedBinding;
    storedBinding = undefined;
  }
}
