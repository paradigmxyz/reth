"use strict"

const {
    CONSTRUCT,
    ReferenceTracker,
} = require("@eslint-community/eslint-utils")
const {
    definePrototypeMethodHandler,
} = require("../util/define-prototype-method-handler")
const { getSourceCode } = require("eslint-compat-utils")

/**
 * @param {Node|undefined} node
 * @returns {boolean}
 */
function isSpreadElement(node) {
    return node && node.type === "SpreadElement"
}

module.exports = {
    meta: {
        docs: {
            description: "disallow resizable and growable ArrayBuffers",
            category: "ES2024",
            recommended: false,
            url: "http://eslint-community.github.io/eslint-plugin-es-x/rules/no-resizable-and-growable-arraybuffers.html",
        },
        fixable: null,
        messages: {
            forbiddenForResizableArrayBuffer:
                "ES2024 Resizable ArrayBuffer is forbidden.",
            forbiddenForGrowableSharedArrayBuffer:
                "ES2024 Growable SharedArrayBuffer is forbidden.",
        },
        schema: [
            {
                type: "object",
                properties: {
                    aggressive: { type: "boolean" },
                },
                additionalProperties: false,
            },
        ],
        type: "problem",
    },
    create(context) {
        return {
            ...definePrototypeMethodHandler(
                context,
                {
                    ArrayBuffer: ["maxByteLength", "resizable", "resize"],
                    SharedArrayBuffer: ["grow", "growable", "maxByteLength"],
                },
                {
                    createReport({ className }) {
                        return {
                            messageId:
                                className === "ArrayBuffer"
                                    ? "forbiddenForResizableArrayBuffer"
                                    : "forbiddenForGrowableSharedArrayBuffer",
                        }
                    },
                },
            ),
            "Program:exit"(program) {
                const sourceCode = getSourceCode(context)
                const tracker = new ReferenceTracker(
                    sourceCode.getScope(program),
                )
                for (const { node, path } of tracker.iterateGlobalReferences({
                    ArrayBuffer: { [CONSTRUCT]: true },
                    SharedArrayBuffer: { [CONSTRUCT]: true },
                })) {
                    if (node.type !== "NewExpression") {
                        continue
                    }
                    const args = node.arguments.slice(0, 2)
                    if (args.some(isSpreadElement)) {
                        continue
                    }
                    const reportedNode = args[1]
                    if (reportedNode) {
                        context.report({
                            node: reportedNode,
                            messageId:
                                path[0] === "ArrayBuffer"
                                    ? "forbiddenForResizableArrayBuffer"
                                    : "forbiddenForGrowableSharedArrayBuffer",
                        })
                    }
                }
            },
        }
    },
}
