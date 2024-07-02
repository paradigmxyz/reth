// @ts-check

/**
 * @typedef {import('prettier').FileInfoOptions} FileInfoOptions
 * @typedef {import('prettier').Options & { onDiskFilepath: string, parserPath?: string, usePrettierrc?: boolean }} Options
 */

const { runAsWorker } = require('synckit');

/**
 * @type {typeof import('prettier')}
 */
let prettier;

runAsWorker(
  /**
   * @param {string} source - The source code to format.
   * @param {Options} options - The prettier options.
   * @param {FileInfoOptions} eslintFileInfoOptions - The file info options.
   * @returns {Promise<string | undefined>} The formatted source code.
   */
  async (
    source,
    {
      filepath,
      onDiskFilepath,
      parserPath,
      usePrettierrc,
      ...eslintPrettierOptions
    },
    eslintFileInfoOptions,
  ) => {
    if (!prettier) {
      prettier = await import('prettier');
    }

    const prettierRcOptions = usePrettierrc
      ? await prettier.resolveConfig(onDiskFilepath, {
          editorconfig: true,
        })
      : null;

    const { ignored, inferredParser } = await prettier.getFileInfo(
      onDiskFilepath,
      {
        resolveConfig: false,
        withNodeModules: false,
        ignorePath: '.prettierignore',
        plugins: /** @type {string[] | undefined} */ (
          prettierRcOptions ? prettierRcOptions.plugins : undefined
        ),
        ...eslintFileInfoOptions,
      },
    );

    // Skip if file is ignored using a .prettierignore file
    if (ignored) {
      return;
    }

    const initialOptions = {};

    // ESLint supports processors that let you extract and lint JS
    // fragments within a non-JS language. In the cases where prettier
    // supports the same language as a processor, we want to process
    // the provided source code as javascript (as ESLint provides the
    // rules with fragments of JS) instead of guessing the parser
    // based off the filename. Otherwise, for instance, on a .md file we
    // end up trying to run prettier over a fragment of JS using the
    // markdown parser, which throws an error.
    // Processors may set virtual filenames for these extracted blocks.
    // If they do so then we want to trust the file extension they
    // provide, and no override is needed.
    // If the processor does not set any virtual filename (signified by
    // `filepath` and `onDiskFilepath` being equal) AND we can't
    // infer the parser from the filename, either because no filename
    // was provided or because there is no parser found for the
    // filename, use javascript.
    // This is added to the options first, so that
    // prettierRcOptions and eslintPrettierOptions can still override
    // the parser.
    //
    // `parserBlocklist` should contain the list of prettier parser
    // names for file types where:
    // * Prettier supports parsing the file type
    // * There is an ESLint processor that extracts JavaScript snippets
    //   from the file type.
    if (filepath === onDiskFilepath) {
      // The following list means the plugin process source into js content
      // but with same filename, so we need to change the parser to `babel`
      // by default.
      // Related ESLint plugins are:
      // 1. `eslint-plugin-graphql` (replacement: `@graphql-eslint/eslint-plugin`)
      // 2. `eslint-plugin-html`
      // 3. `eslint-plugin-markdown@1` (replacement: `eslint-plugin-markdown@2+`)
      // 4. `eslint-plugin-svelte3` (replacement: `eslint-plugin-svelte@2+`)
      const parserBlocklist = [null, 'markdown', 'html'];

      let inferParserToBabel = parserBlocklist.includes(inferredParser);

      switch (inferredParser) {
        // it could be processed by `@graphql-eslint/eslint-plugin` or `eslint-plugin-graphql`
        case 'graphql': {
          if (
            // for `eslint-plugin-graphql`, see https://github.com/apollographql/eslint-plugin-graphql/blob/master/src/index.js#L416
            source.startsWith('ESLintPluginGraphQLFile`')
          ) {
            inferParserToBabel = true;
          }
          break;
        }
        // it could be processed by `@ota-meshi/eslint-plugin-svelte`, `eslint-plugin-svelte` or `eslint-plugin-svelte3`
        case 'svelte': {
          // The `source` would be modified by `eslint-plugin-svelte3`
          if (
            typeof parserPath === 'string' &&
            !parserPath.includes('svelte-eslint-parser')
          ) {
            // We do not support `eslint-plugin-svelte3`,
            // the users should run `prettier` on `.svelte` files manually
            return;
          }
        }
      }

      if (inferParserToBabel) {
        initialOptions.parser = 'babel';
      }
    } else {
      // Similar to https://github.com/prettier/stylelint-prettier/pull/22
      // In all of the following cases ESLint extracts a part of a file to
      // be formatted and there exists a prettier parser for the whole file.
      // If you're interested in prettier you'll want a fully formatted file so
      // you're about to run prettier over the whole file anyway.
      // Therefore running prettier over just the style section is wasteful, so
      // skip it.
      const parserBlocklist = [
        'babel',
        'babylon',
        'flow',
        'typescript',
        'vue',
        'markdown',
        'html',
        'mdx',
        'angular',
        'svelte',
      ];
      if (parserBlocklist.includes(/** @type {string} */ (inferredParser))) {
        return;
      }
    }

    const prettierOptions = {
      ...initialOptions,
      ...prettierRcOptions,
      ...eslintPrettierOptions,
      filepath,
    };

    return prettier.format(source, prettierOptions);
  },
);
