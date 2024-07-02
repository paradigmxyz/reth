"use strict"

const globals = require("globals")
const { commonRules } = require("./_commons")

// eslintrc config: https://eslint.org/docs/latest/use/configure/configuration-files
module.exports.eslintrc = {
    env: {
        node: true,
    },
    globals: {
        ...globals.es2021,
        __dirname: "readonly",
        __filename: "readonly",
        exports: "writable",
        module: "readonly",
        require: "readonly",
    },
    parserOptions: {
        ecmaFeatures: { globalReturn: true },
        ecmaVersion: 2021,
        sourceType: "script",
    },
    rules: {
        ...commonRules,
        "n/no-unsupported-features/es-syntax": ["error", { ignores: [] }],
    },
}

// https://eslint.org/docs/latest/use/configure/configuration-files-new
module.exports.flat = {
    languageOptions: {
        sourceType: "commonjs",
        globals: {
            ...globals.node,
            ...module.exports.eslintrc.globals,
        },
    },
    rules: module.exports.eslintrc.rules,
}
