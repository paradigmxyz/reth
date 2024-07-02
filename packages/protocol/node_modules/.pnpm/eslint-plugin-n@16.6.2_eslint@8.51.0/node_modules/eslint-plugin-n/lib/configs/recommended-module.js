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
        __dirname: "off",
        __filename: "off",
        exports: "off",
        module: "off",
        require: "off",
    },
    parserOptions: {
        ecmaFeatures: { globalReturn: false },
        ecmaVersion: 2021,
        sourceType: "module",
    },
    rules: {
        ...commonRules,
        "n/no-unsupported-features/es-syntax": [
            "error",
            { ignores: ["modules"] },
        ],
    },
}

// flat config: https://eslint.org/docs/latest/use/configure/configuration-files-new
module.exports.flat = {
    languageOptions: {
        sourceType: "module",
        globals: {
            ...globals.node,
            ...module.exports.eslintrc.globals,
        },
    },
    rules: module.exports.eslintrc.rules,
}
