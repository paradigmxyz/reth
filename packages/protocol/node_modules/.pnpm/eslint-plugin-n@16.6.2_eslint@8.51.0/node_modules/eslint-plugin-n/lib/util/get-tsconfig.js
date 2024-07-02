"use strict"

const { getTsconfig, parseTsconfig } = require("get-tsconfig")
const fsCache = new Map()

/**
 * Attempts to get the ExtensionMap from the tsconfig given the path to the tsconfig file.
 *
 * @param {string} filename - The path to the tsconfig.json file
 * @returns {import("get-tsconfig").TsConfigJsonResolved}
 */
function getTSConfig(filename) {
    return parseTsconfig(filename, fsCache)
}

/**
 * Attempts to get the ExtensionMap from the tsconfig of a given file.
 *
 * @param {string} filename - The path to the file we need to find the tsconfig.json of
 * @returns {import("get-tsconfig").TsConfigResult}
 */
function getTSConfigForFile(filename) {
    return getTsconfig(filename, "tsconfig.json", fsCache)
}

module.exports = {
    getTSConfig,
    getTSConfigForFile,
}

module.exports.schema = { type: "string" }
