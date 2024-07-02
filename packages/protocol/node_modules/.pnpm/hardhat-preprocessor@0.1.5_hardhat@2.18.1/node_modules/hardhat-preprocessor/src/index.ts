import './type-extensions';
import {internalTask, extendConfig, task} from 'hardhat/config';
import * as types from 'hardhat/internal/core/params/argumentTypes';
import {TASK_COMPILE_SOLIDITY_READ_FILE} from 'hardhat/builtin-tasks/task-names';
import {
  HardhatConfig,
  HardhatRuntimeEnvironment,
  HardhatUserConfig,
  LinePreprocessor,
  LinePreprocessorConfig,
} from 'hardhat/types';
import murmur128 from 'murmur-128';
import fs from 'fs-extra';
import glob from 'glob';
import {promisify} from 'util';
import path from 'path';
const readdir = promisify<string, string[]>(fs.readdir);
const stat = promisify<string, fs.Stats>(fs.stat);

async function getFiles(dir: string): Promise<string[]> {
  const subdirs: string[] = await readdir(dir);
  const files = await Promise.all(
    subdirs.map(async (subdir) => {
      const res = path.resolve(dir, subdir);
      return (await stat(res)).isDirectory() ? getFiles(res) : [res];
    })
  );
  return files.reduce((a, f) => a.concat(f), []);
}

extendConfig((config: HardhatConfig, userConfig: Readonly<HardhatUserConfig>) => {
  if (userConfig.preprocess) {
    config.preprocess = userConfig.preprocess;
  }
});
// regex copied from https://github.com/ItsNickBarry/hardhat-log-remover
const importsRegex = /\n?(\s*)?import\s*['"]hardhat\/console.sol['"]\s*;/g;
const callsRegex = /\n?((\s|\/)*)?console\s*\.\s*log\w*\s*\([^;]*\)\s*;/g;

export const TASK_PREPROCESS = 'preprocess';

export function removeConsoleLog(
  condition?: (hre: HardhatRuntimeEnvironment) => boolean | Promise<boolean>
): (hre: HardhatRuntimeEnvironment) => Promise<LinePreprocessorConfig | undefined> {
  const preprocess = {
    transform: (line: string, sourceInfo: {absolutePath: string}): string => {
      return line.replace(importsRegex, '').replace(callsRegex, '');
    },
    settings: {removeLog: true},
  };
  return async (hre: HardhatRuntimeEnvironment): Promise<LinePreprocessorConfig | undefined> => {
    if (typeof condition === 'function') {
      const cond = condition(hre);
      const promise = cond as Promise<boolean>;
      if (typeof cond === 'object' && 'then' in promise) {
        return promise.then((v) => (v ? preprocess : undefined));
      } else if (!cond) {
        return Promise.resolve(undefined);
      }
    }
    return Promise.resolve(preprocess);
  };
}

function transform(linePreProcessor: LinePreprocessor, file: {absolutePath: string}, rawContent: string): string {
  return rawContent
    .split(/\r?\n/)
    .map((line) => {
      const newLine = linePreProcessor(line, {absolutePath: file.absolutePath});
      if (newLine.split(/\r?\n/).length > 1) {
        // prevent lines generated to create more line, this ensure preservation of line number while debugging
        throw new Error(`Line processor cannot create new lines. This ensures that line numbers are preserved`);
      }
      return newLine;
    })
    .join('\n');
}

function getAbsolutePathOfFilesToPreprocessFromGlob(specificFiles: string, sources: string): string[] {
  const specificFileList = specificFiles.split(",") || [];
  let filesToPreprocessAbsolutePath: string[] = [];
  if (specificFileList.length > 0) {
    for (let fileNameOrPattern of specificFileList) {
      const filesFound = glob.sync(`${sources}/${fileNameOrPattern}`)
      filesToPreprocessAbsolutePath = filesToPreprocessAbsolutePath.concat(filesFound);
    }
  }
  return filesToPreprocessAbsolutePath;
}

task(TASK_PREPROCESS)
  .addOptionalParam('dest', 'destination folder (default to current sources)', undefined, types.string)
  .addOptionalParam('files', 'specific files/globs to preprocess, comma-delimited - path relative to current sources', undefined, types.string)
  .setAction(async (args, hre) => {
    const linePreProcessor = await getLinePreprocessor(hre);

    const sources = hre.config.paths.sources;
    const destination = args.dest || sources;

    if (linePreProcessor || destination != sources) {
      let filesToPreprocess: string[] = [];

      // if no specific set of files mentioned then get all files.
      // Otherwise iterate through each requested file/glob and retrieve absolute path.
      if(args.files) {
        filesToPreprocess = await getAbsolutePathOfFilesToPreprocessFromGlob(args.files, sources);
      } else { // no specific file mentioned - get all files in sources folder
        filesToPreprocess = await getFiles(sources);
      }

      if (filesToPreprocess.length > 0) {
        await fs.ensureDir(destination);

        for (const file of filesToPreprocess) {
          const from = path.relative(sources, file);
          const to = path.join(destination, from);
          await fs.ensureDir(path.dirname(to));
          const content = fs.readFileSync(file).toString();
          const newContent = linePreProcessor
            ? transform(linePreProcessor.transform, {absolutePath: file}, content)
            : content;
          fs.writeFileSync(to, newContent);
        }
      }
    }
  });

let _linePreprocessor: LinePreprocessorConfig | undefined | null;
async function getLinePreprocessor(hre: HardhatRuntimeEnvironment): Promise<LinePreprocessorConfig | null> {
  if (_linePreprocessor !== undefined) {
    return _linePreprocessor;
  }
  const _getLinePreprocessor = hre.config.preprocess?.eachLine;
  if (_getLinePreprocessor) {
    const linePreProcessorPromise = _getLinePreprocessor(hre);
    if (typeof linePreProcessorPromise === 'object' && 'then' in linePreProcessorPromise) {
      _linePreprocessor = await linePreProcessorPromise;
    } else {
      _linePreprocessor = linePreProcessorPromise;
    }
  }
  return _linePreprocessor || null;
}

// compile task will now preprocess in memory instead of in disk
internalTask(TASK_COMPILE_SOLIDITY_READ_FILE).setAction(
  async ({absolutePath}: {absolutePath: string}, hre, runSuper): Promise<string> => {
    let content = await runSuper({absolutePath});
    const linePreProcessor = await getLinePreprocessor(hre);
    if (linePreProcessor) {
      // if config defined certain files to preprocess -> if current file is not one of them, skip it
      if (linePreProcessor.files) {
        let filesToPreprocess = await getAbsolutePathOfFilesToPreprocessFromGlob(linePreProcessor.files, hre.config.paths.sources)
        filesToPreprocess = filesToPreprocess.map((file) => path.normalize(file));
        if (filesToPreprocess.length > 0 && !filesToPreprocess.includes(absolutePath)) {
          return content;
        }
        // if len(filesToPreprocess) == 0 -> preprocess all files
      }

      let cacheBreaker;
      if (!linePreProcessor.settings) {
        const timeHex = Buffer.from(Date.now().toString()).toString('hex');
        const numCharMissing = 40 - timeHex.length;

        if (numCharMissing > 0) {
          cacheBreaker = '0x' + timeHex.padStart(40, '0');
        }
      } else {
        const settingsString = JSON.stringify(linePreProcessor.settings);
        const settingsHash = Buffer.from(murmur128(settingsString)).toString('hex');
        cacheBreaker = '0x' + settingsHash.padStart(40, '0');
      }

      for (const compiler of hre.config.solidity.compilers) {
        compiler.settings.libraries = compiler.settings.libraries || {};
        compiler.settings.libraries[''] = compiler.settings.libraries[''] || {};
        compiler.settings.libraries[''] = {
          __CACHE_BREAKER__: cacheBreaker,
        };
      }

      content = transform(linePreProcessor.transform, {absolutePath}, content);
    }
    return content;
  }
);
