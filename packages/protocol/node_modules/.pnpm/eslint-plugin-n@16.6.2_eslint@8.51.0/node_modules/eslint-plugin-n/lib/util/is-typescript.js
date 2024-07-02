"use strict"

const path = require("path")

const typescriptExtensions = [".ts", ".tsx", ".cts", ".mts"]

/**
 * Determine if the context source file is typescript.
 *
 * @param {RuleContext} context - A context
 * @returns {boolean}
 */
module.exports = function isTypescript(context) {
    const sourceFileExt = path.extname(
        context.physicalFilename ??
            context.getPhysicalFilename?.() ??
            context.filename ??
            context.getFilename?.()
    )
    return typescriptExtensions.includes(sourceFileExt)
}
