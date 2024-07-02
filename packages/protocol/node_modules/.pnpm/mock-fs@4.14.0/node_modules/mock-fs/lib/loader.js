const {fixWin32Permissions} = require('./item');
const path = require('path');
const FileSystem = require('./filesystem');
const fs = require('fs');
const bypass = require('./bypass');

const createContext = ({output, options = {}, target}, newContext) =>
  Object.assign(
    {
      // Assign options and set defaults if needed
      options: {
        recursive: options.recursive !== false,
        lazy: options.lazy !== false
      },
      output,
      target
    },
    newContext
  );

function addFile(context, stats, isRoot) {
  const {output, target} = context;
  const {lazy} = context.options;

  if (!stats.isFile()) {
    throw new Error(`${target} is not a valid file!`);
  }

  const outputPropKey = isRoot ? target : path.basename(target);

  output[outputPropKey] = () => {
    const content = !lazy ? fs.readFileSync(target) : '';
    const file = FileSystem.file(Object.assign({}, stats, {content}))();

    if (lazy) {
      Object.defineProperty(file, '_content', {
        get() {
          const res = bypass(() => fs.readFileSync(target));
          Object.defineProperty(file, '_content', {
            value: res,
            writable: true
          });
          return res;
        },
        set(data) {
          Object.defineProperty(file, '_content', {
            value: data,
            writable: true
          });
        },
        configurable: true
      });
    }

    return file;
  };

  return output[outputPropKey];
}

function addDir(context, stats, isRoot) {
  const {target, output} = context;
  const {recursive} = context.options;

  if (!stats.isDirectory()) {
    throw new Error(`${target} is not a valid directory!`);
  }

  stats = Object.assign({}, stats);
  const outputPropKey = isRoot ? target : path.basename(target);

  // On windows platforms, directories do not have the executable flag, which causes FileSystem.prototype.getItem
  // to think that the directory cannot be traversed. This is a workaround, however, a better solution may be to
  // re-think the logic in FileSystem.prototype.getItem
  // This workaround adds executable privileges if read privileges are found
  stats.mode = fixWin32Permissions(stats.mode);

  // Create directory factory
  const directoryItems = {};
  output[outputPropKey] = FileSystem.directory(
    Object.assign(stats, {items: directoryItems})
  );

  fs.readdirSync(target).forEach(p => {
    const absPath = path.join(target, p);
    const stats = fs.statSync(absPath);
    const newContext = createContext(context, {
      target: absPath,
      output: directoryItems
    });

    if (recursive && stats.isDirectory()) {
      addDir(newContext, stats);
    } else if (stats.isFile()) {
      addFile(newContext, stats);
    }
  });

  return output[outputPropKey];
}

/**
 * Load directory or file from real FS
 */
exports.load = function(p, options) {
  return bypass(() => {
    p = path.resolve(p);

    const stats = fs.statSync(p);
    const context = createContext({output: {}, options, target: p});

    if (stats.isDirectory()) {
      return addDir(context, stats, true);
    } else if (stats.isFile()) {
      return addFile(context, stats, true);
    }
  });
};
