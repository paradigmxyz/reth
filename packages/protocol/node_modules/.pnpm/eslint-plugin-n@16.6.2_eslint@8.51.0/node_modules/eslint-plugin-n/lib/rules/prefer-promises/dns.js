/**
 * @author Toru Nagashima
 * See LICENSE file in root directory for full license.
 */
"use strict"

const {
    CALL,
    CONSTRUCT,
    ReferenceTracker,
} = require("@eslint-community/eslint-utils")

const trackMap = {
    dns: {
        lookup: { [CALL]: true },
        lookupService: { [CALL]: true },
        Resolver: { [CONSTRUCT]: true },
        getServers: { [CALL]: true },
        resolve: { [CALL]: true },
        resolve4: { [CALL]: true },
        resolve6: { [CALL]: true },
        resolveAny: { [CALL]: true },
        resolveCname: { [CALL]: true },
        resolveMx: { [CALL]: true },
        resolveNaptr: { [CALL]: true },
        resolveNs: { [CALL]: true },
        resolvePtr: { [CALL]: true },
        resolveSoa: { [CALL]: true },
        resolveSrv: { [CALL]: true },
        resolveTxt: { [CALL]: true },
        reverse: { [CALL]: true },
        setServers: { [CALL]: true },
    },
}
trackMap["node:dns"] = trackMap.dns

module.exports = {
    meta: {
        docs: {
            description: 'enforce `require("dns").promises`',
            recommended: false,
            url: "https://github.com/eslint-community/eslint-plugin-n/blob/HEAD/docs/rules/prefer-promises/dns.md",
        },
        fixable: null,
        messages: {
            preferPromises: "Use 'dns.promises.{{name}}()' instead.",
            preferPromisesNew: "Use 'new dns.promises.{{name}}()' instead.",
        },
        schema: [],
        type: "suggestion",
    },

    create(context) {
        return {
            "Program:exit"(node) {
                const sourceCode = context.sourceCode ?? context.getSourceCode() // TODO: just use context.sourceCode when dropping eslint < v9
                const scope = sourceCode.getScope?.(node) ?? context.getScope() //TODO: remove context.getScope() when dropping support for ESLint < v9
                const tracker = new ReferenceTracker(scope, { mode: "legacy" })
                const references = [
                    ...tracker.iterateCjsReferences(trackMap),
                    ...tracker.iterateEsmReferences(trackMap),
                ]

                for (const { node, path } of references) {
                    const name = path[path.length - 1]
                    const isClass = name[0] === name[0].toUpperCase()
                    context.report({
                        node,
                        messageId: isClass
                            ? "preferPromisesNew"
                            : "preferPromises",
                        data: { name },
                    })
                }
            },
        }
    },
}
