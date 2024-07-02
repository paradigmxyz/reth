/*!
 * delete-empty <https://github.com/jonschlinkert/delete-empty>
 * Copyright (c) 2015-present, Jon Schlinkert
 * Released under the MIT License.
 */

'use strict';

const fs = require('fs');
const util = require('util');
const path = require('path');
const rimraf = require('rimraf');
const startsWith = require('path-starts-with');
const colors = require('ansi-colors');
const readdir = util.promisify(fs.readdir);

/**
 * Helpers
 */

const GARBAGE_REGEX = /(?:Thumbs\.db|\.DS_Store)$/i;
const isGarbageFile = (file, regex = GARBAGE_REGEX) => regex.test(file);
const filterGarbage = (file, regex) => !isGarbageFile(file, regex);
const isValidDir = (cwd, dir, empty) => {
  return !empty.includes(dir) && startsWith(dir, cwd) && isDirectory(dir);
};

const deleteDir = async (dirname, options = {}) => {
  if (options.dryRun !== true) {
    return new Promise((resolve, reject) => {
      rimraf(dirname, { ...options, glob: false }, err => {
        if (err) {
          reject(err);
        } else {
          resolve();
        }
      });
    })
  }
};

const deleteDirSync = (dirname, options = {}) => {
  if (options.dryRun !== true) {
    return rimraf.sync(dirname, { ...options, glob: false });
  }
};

const deleteEmpty = (cwd, options, cb) => {
  if (typeof cwd !== 'string') {
    return Promise.reject(new TypeError('expected the first argument to be a string'));
  }

  if (typeof options === 'function') {
    cb = options;
    options = null;
  }

  if (typeof cb === 'function') {
    return deleteEmpty(cwd, options)
      .then(res => cb(null, res))
      .catch(cb);
  }

  const opts = options || {};
  const dirname = path.resolve(cwd);
  const onDirectory = opts.onDirectory || (() => {});
  const empty = [];

  const remove = async filepath => {
    let dir = path.resolve(filepath);

    if (!isValidDir(cwd, dir, empty)) return;
    onDirectory(dir);

    let files = await readdir(dir);

    if (isEmpty(dir, files, empty, opts)) {
      empty.push(dir);

      await deleteDir(dir, opts);

      if (opts.verbose === true) {
        console.log(colors.red('Deleted:'), path.relative(cwd, dir));
      }

      if (typeof opts.onDelete === 'function') {
        await opts.onDelete(dir);
      }

      return remove(path.dirname(dir));
    }

    for (const file of files) {
      await remove(path.join(dir, file));
    }

    return empty;
  };

  return remove(dirname);
};

deleteEmpty.sync = (cwd, options) => {
  if (typeof cwd !== 'string') {
    throw new TypeError('expected the first argument to be a string');
  }

  const opts = options || {};
  const dirname = path.resolve(cwd);
  const deleted = [];
  const empty = [];

  const remove = filepath => {
    let dir = path.resolve(filepath);

    if (!isValidDir(cwd, dir, empty)) {
      return empty;
    }

    let files = fs.readdirSync(dir);

    if (isEmpty(dir, files, empty, opts)) {
      empty.push(dir);

      deleteDirSync(dir, opts);

      if (opts.verbose === true) {
        console.log(colors.red('Deleted:'), path.relative(cwd, dir));
      }

      if (typeof opts.onDelete === 'function') {
        opts.onDelete(dir);
      }

      return remove(path.dirname(dir));
    }

    for (let filepath of files) {
      remove(path.join(dir, filepath));
    }
    return empty;
  };

  remove(dirname);
  return empty;
};

/**
 * Return true if the given `files` array has zero length or only
 * includes unwanted files.
 */

const isEmpty = (dir, files, empty, options = {}) => {
  let filter = options.filter || filterGarbage;
  let regex = options.junkRegex;

  for (let basename of files) {
    let filepath = path.join(dir, basename);

    if (!(options.dryRun && empty.includes(filepath)) && filter(filepath, regex) === true) {
      return false;
    }
  }
  return true;
};

/**
 * Returns true if the given filepath exists and is a directory
 */

const isDirectory = dir => {
  try {
    return fs.statSync(dir).isDirectory();
  } catch (err) { /* do nothing */ }
  return false;
};

/**
 * Expose deleteEmpty
 */

module.exports = deleteEmpty;
