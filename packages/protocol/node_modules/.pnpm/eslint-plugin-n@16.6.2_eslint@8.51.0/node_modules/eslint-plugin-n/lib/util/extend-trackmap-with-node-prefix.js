"use strict"

const isCoreModule = require("is-core-module")

/**
 * Extend trackMap.modules with `node:` prefixed modules
 * @param {Object} modules Like `{assert: foo}`
 * @returns {Object} Like `{assert: foo}, "node:assert": foo}`
 */
module.exports = function extendTrackMapWithNodePrefix(modules) {
    const ret = {
        ...modules,
        ...Object.fromEntries(
            Object.entries(modules)
                .map(([name, value]) => [`node:${name}`, value])
                // Note: "999" arbitrary to check current/future Node.js version
                .filter(([name]) => isCoreModule(name, "999"))
        ),
    }
    return ret
}
