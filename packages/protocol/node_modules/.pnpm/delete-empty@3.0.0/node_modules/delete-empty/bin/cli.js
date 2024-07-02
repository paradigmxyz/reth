#!/usr/bin/env node

const os = require('os');
const path = require('path');
const { cyan, bold, dim, green, symbols } = require('ansi-colors');
const deleteEmpty = require('..');
const argv = require('minimist')(process.argv.slice(2), {
  boolean: true,
  number: true,
  alias: { d: 'dryRun' },
  rename: { _: 'files' }
});

const moduleDir = dim(`<${path.dirname(__dirname)}>`);
const name = pkg => bold(`delete-empty v${pkg.version}`);
const help = pkg => `
Path: <${path.dirname(__dirname)}>

Usage: ${cyan('$ delete-empty <directory> [options]')}

Directory: (optional) Initial directory to begin the search for empty
           directories. Otherwise, cwd is used.

[Options]:
  -c, --cwd           Set the current working directory for folders to search.
  -d, --dryRun        Do a dry run without deleting any files.
  -h, --help          Display this help menu
  -V, --version       Display the current version of rename
  -v, --verbose       Display all verbose logging messages (currently not used)
`;

if (argv.help) {
  console.log(help(require('../package')));
  process.exit();
}

const ok = green(symbols.check);
const cwd = path.resolve(argv._[0] || argv.cwd || process.cwd());
const relative = filepath => {
  if (filepath.startsWith(cwd)) {
    return path.relative(cwd, filepath);
  }
  if (filepath.startsWith(os.homedir())) {
    return path.join('~', filepath.slice(os.homedir().length));
  }
  return cwd;
};

if (argv.dryRun) {
  argv.verbose = true;
}

let count = 0;
argv.onDirectory = () => (count++);

deleteEmpty(cwd, argv)
  .then(files => {
    console.log(ok, 'Deleted', files.length, 'of', count, 'directories');
    process.exit();
  })
  .catch(err => {
    console.error(err);
    process.exit(1);
  });
