"use strict"

/**
 * Remove `node:` prefix from module name
 * @param {string} name The module name such as `node:assert` or `assert`.
 * @returns {string} The unprefixed module name like `assert`.
 */
module.exports = function unprefixNodeColon(name) {
    if (name.startsWith("node:")) {
        return name.slice(5)
    }
    return name
}
