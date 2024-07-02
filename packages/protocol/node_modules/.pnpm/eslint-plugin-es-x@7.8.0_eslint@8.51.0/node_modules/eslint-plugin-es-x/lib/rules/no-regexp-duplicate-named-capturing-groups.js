"use strict"

const { getSourceCode } = require("eslint-compat-utils")
const { defineRegExpHandler } = require("../util/define-regexp-handler")

module.exports = {
    meta: {
        docs: {
            description: "disallow RegExp duplicate named capturing groups.",
            category: "ES2025",
            recommended: false,
            url: "http://eslint-community.github.io/eslint-plugin-es-x/rules/no-regexp-duplicate-named-capturing-groups.html",
        },
        fixable: null,
        messages: {
            forbidden:
                "ES2025 RegExp duplicate named capturing groups are forbidden.",
        },
        schema: [],
        type: "problem",
    },
    create(context) {
        return defineRegExpHandler(context, (node) => {
            const found = new Map()
            return {
                onPatternEnter() {
                    found.clear()
                },
                onCapturingGroupLeave(start, end, name) {
                    if (!name) {
                        return
                    }
                    const list = found.get(name)
                    if (list) {
                        list.push({ start, end })
                    } else {
                        found.set(name, [{ start, end }])
                    }
                },
                onExit() {
                    for (const [, dupe] of found.values()) {
                        if (!dupe) {
                            continue
                        }
                        const { start, end } = dupe
                        const sourceCode = getSourceCode(context)
                        context.report({
                            node,
                            loc:
                                node.type === "Literal"
                                    ? {
                                          start: sourceCode.getLocFromIndex(
                                              node.range[0] +
                                                  1 /* slash */ +
                                                  start,
                                          ),
                                          end: sourceCode.getLocFromIndex(
                                              node.range[0] +
                                                  1 /* slash */ +
                                                  end,
                                          ),
                                      }
                                    : null,
                            messageId: "forbidden",
                        })
                    }
                },
            }
        })
    },
}
