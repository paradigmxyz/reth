/**
 * @author Toru Nagashima
 * See LICENSE file in root directory for full license.
 */
"use strict"

const { checkExistence, messages } = require("../util/check-existence")
const getAllowModules = require("../util/get-allow-modules")
const getResolvePaths = require("../util/get-resolve-paths")
const getTSConfig = require("../util/get-tsconfig")
const getTryExtensions = require("../util/get-try-extensions")
const getTypescriptExtensionMap = require("../util/get-typescript-extension-map")
const visitRequire = require("../util/visit-require")

module.exports = {
    meta: {
        docs: {
            description:
                "disallow `require()` expressions which import non-existence modules",
            recommended: true,
            url: "https://github.com/eslint-community/eslint-plugin-n/blob/HEAD/docs/rules/no-missing-require.md",
        },
        type: "problem",
        fixable: null,
        schema: [
            {
                type: "object",
                properties: {
                    allowModules: getAllowModules.schema,
                    tryExtensions: getTryExtensions.schema,
                    resolvePaths: getResolvePaths.schema,
                    typescriptExtensionMap: getTypescriptExtensionMap.schema,
                    tsconfigPath: getTSConfig.schema,
                },
                additionalProperties: false,
            },
        ],
        messages,
    },
    create(context) {
        const filePath = context.filename ?? context.getFilename()
        if (filePath === "<input>") {
            return {}
        }

        return visitRequire(context, {}, targets => {
            checkExistence(context, targets)
        })
    },
}
