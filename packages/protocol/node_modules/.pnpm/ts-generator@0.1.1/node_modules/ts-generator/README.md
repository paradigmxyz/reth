# ts-generator

The missing piece for fully typesafe Typescript apps

Generate Typescript typings for any kind of files (json, graphql queries, css modules).

[![Coverage Status](https://coveralls.io/repos/github/krzkaczor/ts-generator/badge.svg?branch=master)](https://coveralls.io/github/krzkaczor/ts-gen?branch=master)

## Features:

- plugin based architecture allowing one tool to handle all different "untyped" data (css modules, graphql queries,
  ethereum smartcontracts, json files) — think babel for types
- unified config for all plugins
- automatically prettifies output to match code style of the project
- watch mode

## Example:

- [generate types for simple JSON files](https://github.com/krzkaczor/ts-generator/blob/master/test/integration/ts-gen-plugins/dummy-json/index.ts)
