/**
 * @author Toru Nagashima
 * See LICENSE file in root directory for full license.
 */
"use strict"

const path = require("path")
const fs = require("fs")
const mapTypescriptExtension = require("../util/map-typescript-extension")
const visitImport = require("../util/visit-import")

/**
 * Get all file extensions of the files which have the same basename.
 * @param {string} filePath The path to the original file to check.
 * @returns {string[]} File extensions.
 */
function getExistingExtensions(filePath) {
    const basename = path.basename(filePath, path.extname(filePath))
    try {
        return fs
            .readdirSync(path.dirname(filePath))
            .filter(
                filename =>
                    path.basename(filename, path.extname(filename)) === basename
            )
            .map(filename => path.extname(filename))
    } catch (_error) {
        return []
    }
}

module.exports = {
    meta: {
        docs: {
            description:
                "enforce the style of file extensions in `import` declarations",
            recommended: false,
            url: "https://github.com/eslint-community/eslint-plugin-n/blob/HEAD/docs/rules/file-extension-in-import.md",
        },
        fixable: "code",
        messages: {
            requireExt: "require file extension '{{ext}}'.",
            forbidExt: "forbid file extension '{{ext}}'.",
        },
        schema: [
            {
                enum: ["always", "never"],
            },
            {
                type: "object",
                properties: {},
                additionalProperties: {
                    enum: ["always", "never"],
                },
            },
        ],
        type: "suggestion",
    },
    create(context) {
        if ((context.filename ?? context.getFilename()).startsWith("<")) {
            return {}
        }
        const defaultStyle = context.options[0] || "always"
        const overrideStyle = context.options[1] || {}

        /**
         * @param {import("../util/import-target.js")} target
         * @returns {void}
         */
        function verify({ filePath, name, node, moduleType }) {
            // Ignore if it's not resolved to a file or it's a bare module.
            if (moduleType !== "relative" && moduleType !== "absolute") {
                return
            }

            // Get extension.
            const originalExt = path.extname(name)
            const existingExts = getExistingExtensions(filePath)
            const ext = path.extname(filePath) || existingExts.join(" or ")
            const style = overrideStyle[ext] || defaultStyle

            // Verify.
            if (style === "always" && ext !== originalExt) {
                const fileExtensionToAdd = mapTypescriptExtension(
                    context,
                    filePath,
                    ext
                )
                context.report({
                    node,
                    messageId: "requireExt",
                    data: { ext: fileExtensionToAdd },
                    fix(fixer) {
                        if (existingExts.length !== 1) {
                            return null
                        }
                        const index = node.range[1] - 1
                        return fixer.insertTextBeforeRange(
                            [index, index],
                            fileExtensionToAdd
                        )
                    },
                })
            } else if (style === "never" && ext === originalExt) {
                context.report({
                    node,
                    messageId: "forbidExt",
                    data: { ext },
                    fix(fixer) {
                        if (existingExts.length !== 1) {
                            return null
                        }
                        const index = name.lastIndexOf(ext)
                        const start = node.range[0] + 1 + index
                        const end = start + ext.length
                        return fixer.removeRange([start, end])
                    },
                })
            }
        }

        return visitImport(context, { optionIndex: 1 }, targets => {
            targets.forEach(verify)
        })
    },
}
