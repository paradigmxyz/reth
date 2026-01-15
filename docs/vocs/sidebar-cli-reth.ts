import { SidebarItem } from "vocs";

export const rethCliSidebar: SidebarItem = {
    text: "reth",
    link: "/cli/reth",
    collapsed: false,
    items: [
        {
            text: "reth node",
            link: "/cli/reth/node"
        },
        {
            text: "reth init",
            link: "/cli/reth/init"
        },
        {
            text: "reth init-state",
            link: "/cli/reth/init-state"
        },
        {
            text: "reth import",
            link: "/cli/reth/import"
        },
        {
            text: "reth import-era",
            link: "/cli/reth/import-era"
        },
        {
            text: "reth export-era",
            link: "/cli/reth/export-era"
        },
        {
            text: "reth dump-genesis",
            link: "/cli/reth/dump-genesis"
        },
        {
            text: "reth db",
            link: "/cli/reth/db",
            collapsed: true,
            items: [
                {
                    text: "reth db stats",
                    link: "/cli/reth/db/stats"
                },
                {
                    text: "reth db list",
                    link: "/cli/reth/db/list"
                },
                {
                    text: "reth db checksum",
                    link: "/cli/reth/db/checksum"
                },
                {
                    text: "reth db diff",
                    link: "/cli/reth/db/diff"
                },
                {
                    text: "reth db get",
                    link: "/cli/reth/db/get",
                    collapsed: true,
                    items: [
                        {
                            text: "reth db get mdbx",
                            link: "/cli/reth/db/get/mdbx"
                        },
                        {
                            text: "reth db get static-file",
                            link: "/cli/reth/db/get/static-file"
                        }
                    ]
                },
                {
                    text: "reth db drop",
                    link: "/cli/reth/db/drop"
                },
                {
                    text: "reth db clear",
                    link: "/cli/reth/db/clear",
                    collapsed: true,
                    items: [
                        {
                            text: "reth db clear mdbx",
                            link: "/cli/reth/db/clear/mdbx"
                        },
                        {
                            text: "reth db clear static-file",
                            link: "/cli/reth/db/clear/static-file"
                        }
                    ]
                },
                {
                    text: "reth db repair-trie",
                    link: "/cli/reth/db/repair-trie"
                },
                {
                    text: "reth db static-file-header",
                    link: "/cli/reth/db/static-file-header",
                    collapsed: true,
                    items: [
                        {
                            text: "reth db static-file-header block",
                            link: "/cli/reth/db/static-file-header/block"
                        },
                        {
                            text: "reth db static-file-header path",
                            link: "/cli/reth/db/static-file-header/path"
                        }
                    ]
                },
                {
                    text: "reth db version",
                    link: "/cli/reth/db/version"
                },
                {
                    text: "reth db path",
                    link: "/cli/reth/db/path"
                },
                {
                    text: "reth db settings",
                    link: "/cli/reth/db/settings",
                    collapsed: true,
                    items: [
                        {
                            text: "reth db settings get",
                            link: "/cli/reth/db/settings/get"
                        },
                        {
                            text: "reth db settings set",
                            link: "/cli/reth/db/settings/set",
                            collapsed: true,
                            items: [
                                {
                                    text: "reth db settings set receipts",
                                    link: "/cli/reth/db/settings/set/receipts"
                                },
                                {
                                    text: "reth db settings set transaction_senders",
                                    link: "/cli/reth/db/settings/set/transaction_senders"
                                },
                                {
                                    text: "reth db settings set account_changesets",
                                    link: "/cli/reth/db/settings/set/account_changesets"
                                },
                                {
                                    text: "reth db settings set storage_changesets",
                                    link: "/cli/reth/db/settings/set/storage_changesets"
                                }
                            ]
                        }
                    ]
                },
                {
                    text: "reth db account-storage",
                    link: "/cli/reth/db/account-storage"
                }
            ]
        },
        {
            text: "reth download",
            link: "/cli/reth/download"
        },
        {
            text: "reth stage",
            link: "/cli/reth/stage",
            collapsed: true,
            items: [
                {
                    text: "reth stage run",
                    link: "/cli/reth/stage/run"
                },
                {
                    text: "reth stage drop",
                    link: "/cli/reth/stage/drop"
                },
                {
                    text: "reth stage dump",
                    link: "/cli/reth/stage/dump",
                    collapsed: true,
                    items: [
                        {
                            text: "reth stage dump execution",
                            link: "/cli/reth/stage/dump/execution"
                        },
                        {
                            text: "reth stage dump storage-hashing",
                            link: "/cli/reth/stage/dump/storage-hashing"
                        },
                        {
                            text: "reth stage dump account-hashing",
                            link: "/cli/reth/stage/dump/account-hashing"
                        },
                        {
                            text: "reth stage dump merkle",
                            link: "/cli/reth/stage/dump/merkle"
                        }
                    ]
                },
                {
                    text: "reth stage unwind",
                    link: "/cli/reth/stage/unwind",
                    collapsed: true,
                    items: [
                        {
                            text: "reth stage unwind to-block",
                            link: "/cli/reth/stage/unwind/to-block"
                        },
                        {
                            text: "reth stage unwind num-blocks",
                            link: "/cli/reth/stage/unwind/num-blocks"
                        }
                    ]
                }
            ]
        },
        {
            text: "reth p2p",
            link: "/cli/reth/p2p",
            collapsed: true,
            items: [
                {
                    text: "reth p2p header",
                    link: "/cli/reth/p2p/header"
                },
                {
                    text: "reth p2p body",
                    link: "/cli/reth/p2p/body"
                },
                {
                    text: "reth p2p rlpx",
                    link: "/cli/reth/p2p/rlpx",
                    collapsed: true,
                    items: [
                        {
                            text: "reth p2p rlpx ping",
                            link: "/cli/reth/p2p/rlpx/ping"
                        }
                    ]
                },
                {
                    text: "reth p2p bootnode",
                    link: "/cli/reth/p2p/bootnode"
                }
            ]
        },
        {
            text: "reth config",
            link: "/cli/reth/config"
        },
        {
            text: "reth prune",
            link: "/cli/reth/prune"
        },
        {
            text: "reth re-execute",
            link: "/cli/reth/re-execute"
        }
    ]
};
