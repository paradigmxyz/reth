/**
 * @author Matt DuVall<http://mattduvall.com/>
 * See LICENSE file in root directory for full license.
 */
"use strict"

const allowedAtRootLevelSelector = [
    // fs.readFileSync()
    ":function MemberExpression > Identifier[name=/Sync$/]",
    // readFileSync.call(null, 'path')
    ":function MemberExpression > Identifier[name=/Sync$/]",
    // readFileSync()
    ":function :not(MemberExpression) > Identifier[name=/Sync$/]",
]

const disallowedAtRootLevelSelector = [
    // fs.readFileSync()
    "MemberExpression > Identifier[name=/Sync$/]",
    // readFileSync.call(null, 'path')
    "MemberExpression > Identifier[name=/Sync$/]",
    // readFileSync()
    ":not(MemberExpression) > Identifier[name=/Sync$/]",
]

module.exports = {
    meta: {
        type: "suggestion",
        docs: {
            description: "disallow synchronous methods",
            recommended: false,
            url: "https://github.com/eslint-community/eslint-plugin-n/blob/HEAD/docs/rules/no-sync.md",
        },
        fixable: null,
        schema: [
            {
                type: "object",
                properties: {
                    allowAtRootLevel: {
                        type: "boolean",
                        default: false,
                    },
                },
                additionalProperties: false,
            },
        ],
        messages: {
            noSync: "Unexpected sync method: '{{propertyName}}'.",
        },
    },

    create(context) {
        const selector = context.options[0]?.allowAtRootLevel
            ? allowedAtRootLevelSelector
            : disallowedAtRootLevelSelector

        return {
            [selector](node) {
                context.report({
                    node: node.parent,
                    messageId: "noSync",
                    data: {
                        propertyName: node.name,
                    },
                })
            },
        }
    },
}
