import { SidebarItem } from "vocs";

export const sidebar: SidebarItem[] = [
    {
        text: "Introduction",
        items: [
            {
                text: "Overview",
                link: "/overview"
            },
            {
                text: "Why Reth?",
                link: "/introduction/why-reth"
            },
            {
                text: "Contributing",
                link: "/introduction/contributing"
            }
        ]
    },
    {
        text: "Reth for Node Operators",
        items: [
            {
                text: "System Requirements",
                link: "/run/system-requirements"
            },
            {
                text: "Installation",
                collapsed: true,
                items: [
                    {
                        text: "Overview",
                        link: "/installation/overview"
                    },
                    {
                        text: "Pre-Built Binaries",
                        link: "/installation/binaries"
                    },
                    {
                        text: "Docker",
                        link: "/installation/docker"
                    },
                    {
                        text: "Build from Source",
                        link: "/installation/source"
                    },
                    {
                        text: "Build for ARM devices",
                        link: "/installation/build-for-arm-devices"
                    },
                    {
                        text: "Update Priorities",
                        link: "/installation/priorities"
                    }
                ]
            },
            {
                text: "Running a Node",
                items: [
                    {
                        text: "Overview",
                        link: "/run/overview",
                    },
                    {
                        text: "Networks",
                        // link: "/run/networks",
                        items: [
                            {
                                text: "Ethereum",
                                link: "/run/ethereum",
                                // items: [
                                //     {
                                //         text: "Snapshots",
                                //         link: "/run/ethereum/snapshots"
                                //     }
                                // ]
                            },
                            {
                                text: "OP-stack",
                                link: "/run/opstack",
                                // items: [
                                //     {
                                //         text: "Caveats OP-Mainnet",
                                //         link: "/run/opstack/op-mainnet-caveats"
                                //     }
                                // ]
                            },
                            {
                                text: "Private testnets",
                                link: "/run/private-testnets"
                            }
                        ]
                    },
                ]
            },
            {
                text: "Configuration",
                link: "/run/configuration"
            },
            {
                text: "Monitoring",
                link: "/run/monitoring"
            },
            {
                text: "FAQ",
                link: "/run/faq",
                collapsed: true,
                items: [
                    {
                        text: "Transaction Types",
                        link: "/run/faq/transactions"
                    },
                    {
                        text: "Pruning & Full Node",
                        link: "/run/faq/pruning"
                    },
                    {
                        text: "Ports",
                        link: "/run/faq/ports"
                    },
                    {
                        text: "Profiling",
                        link: "/run/faq/profiling"
                    },
                    {
                        text: "Sync OP Mainnet",
                        link: "/run/faq/sync-op-mainnet"
                    }
                ]
            }
        ]
    },
    {
        text: "Reth as a library",
        items: [
            {
                text: "Overview",
                link: "/sdk"
            },
            {
                text: "Typesystem",
                items: [
                    {
                        text: "Block",
                        link: "/sdk/typesystem/block"
                    },
                    {
                        text: "Transaction types",
                        link: "/sdk/typesystem/transaction-types"
                    }
                ]
            },
            {
                text: "What is in a node?",
                collapsed: false,
                items: [
                    {
                        text: "Network",
                        link: "/sdk/node-components/network"
                    },
                    {
                        text: "Pool",
                        link: "/sdk/node-components/pool"
                    },
                    {
                        text: "Consensus",
                        link: "/sdk/node-components/consensus"
                    },
                    {
                        text: "EVM",
                        link: "/sdk/node-components/evm"
                    },
                    {
                        text: "RPC",
                        link: "/sdk/node-components/rpc"
                    }
                ]
            },
            // TODO
            // {
            //     text: "Build a custom node",
            //     items: [
            //         {
            //             text: "Prerequisites and Considerations",
            //             link: "/sdk/custom-node/prerequisites"
            //         },
            //         {
            //             text: "What modifications and how",
            //             link: "/sdk/custom-node/modifications"
            //         }
            //     ]
            // },
            // {
            //     text: "Examples",
            //     items: [
            //         {
            //             text: "How to modify an existing node",
            //             items: [
            //                 {
            //                     text: "Additional features: RPC endpoints, services",
            //                     link: "/sdk/examples/modify-node"
            //                 }
            //             ]
            //         },
            //         {
            //             text: "How to use standalone components",
            //             items: [
            //                 {
            //                     text: "Interact with the disk directly + caveats",
            //                     link: "/sdk/examples/standalone-components"
            //                 }
            //             ]
            //         }
            //     ]
            // }
        ]
    },
    {
        text: "Execution Extensions",
        items: [
            {
                text: "Overview",
                link: "/exex/overview"
            },
            {
                text: "How do ExExes work?",
                link: "/exex/how-it-works"
            },
            {
                text: "Hello World",
                link: "/exex/hello-world"
            },
            {
                text: "Tracking State",
                link: "/exex/tracking-state"
            },
            {
                text: "Remote",
                link: "/exex/remote"
            }
        ]
    },
    {
        text: "Interacting with Reth over JSON-RPC",
        
        items: [
            {
                text: "Overview",
                link: "/jsonrpc/intro",
            },
            {
                text: "eth",
                link: "/jsonrpc/eth"
            },
            {
                text: "web3",
                link: "/jsonrpc/web3"
            },
            {
                text: "net",
                link: "/jsonrpc/net"
            },
            {
                text: "txpool",
                link: "/jsonrpc/txpool"
            },
            {
                text: "debug",
                link: "/jsonrpc/debug"
            },
            {
                text: "trace",
                link: "/jsonrpc/trace"
            },
            {
                text: "admin",
                link: "/jsonrpc/admin"
            },
            {
                text: "rpc",
                link: "/jsonrpc/rpc"
            }
        ]
    },
    {
        text: "CLI Reference",
        link: "/cli/cli",
        collapsed: false,
        items: [
            {
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
                                                text: "reth db settings set receipts_in_static_files",
                                                link: "/cli/reth/db/settings/set/receipts_in_static_files"
                                            },
                                            {
                                                text: "reth db settings set transaction_senders_in_static_files",
                                                link: "/cli/reth/db/settings/set/transaction_senders_in_static_files"
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
            },
            {
                text: "op-reth",
                link: "/cli/op-reth",
                collapsed: false,
                items: [
                    {
                        text: "op-reth node",
                        link: "/cli/op-reth/node"
                    },
                    {
                        text: "op-reth init",
                        link: "/cli/op-reth/init"
                    },
                    {
                        text: "op-reth init-state",
                        link: "/cli/op-reth/init-state"
                    },
                    {
                        text: "op-reth import-op",
                        link: "/cli/op-reth/import-op"
                    },
                    {
                        text: "op-reth import-receipts-op",
                        link: "/cli/op-reth/import-receipts-op"
                    },
                    {
                        text: "op-reth dump-genesis",
                        link: "/cli/op-reth/dump-genesis"
                    },
                    {
                        text: "op-reth db",
                        link: "/cli/op-reth/db",
                        collapsed: true,
                        items: [
                            {
                                text: "op-reth db stats",
                                link: "/cli/op-reth/db/stats"
                            },
                            {
                                text: "op-reth db list",
                                link: "/cli/op-reth/db/list"
                            },
                            {
                                text: "op-reth db checksum",
                                link: "/cli/op-reth/db/checksum"
                            },
                            {
                                text: "op-reth db diff",
                                link: "/cli/op-reth/db/diff"
                            },
                            {
                                text: "op-reth db get",
                                link: "/cli/op-reth/db/get",
                                collapsed: true,
                                items: [
                                    {
                                        text: "op-reth db get mdbx",
                                        link: "/cli/op-reth/db/get/mdbx"
                                    },
                                    {
                                        text: "op-reth db get static-file",
                                        link: "/cli/op-reth/db/get/static-file"
                                    }
                                ]
                            },
                            {
                                text: "op-reth db drop",
                                link: "/cli/op-reth/db/drop"
                            },
                            {
                                text: "op-reth db clear",
                                link: "/cli/op-reth/db/clear",
                                collapsed: true,
                                items: [
                                    {
                                        text: "op-reth db clear mdbx",
                                        link: "/cli/op-reth/db/clear/mdbx"
                                    },
                                    {
                                        text: "op-reth db clear static-file",
                                        link: "/cli/op-reth/db/clear/static-file"
                                    }
                                ]
                            },
                            {
                                text: "op-reth db version",
                                link: "/cli/op-reth/db/version"
                            },
                            {
                                text: "op-reth db path",
                                link: "/cli/op-reth/db/path"
                            },
                            {
                                text: "op-reth db account-storage",
                                link: "/cli/op-reth/db/account-storage"
                            },
                            {
                                text: "op-reth db repair-trie",
                                link: "/cli/op-reth/db/repair-trie"
                            },
                            {
                                text: "op-reth db settings",
                                link: "/cli/op-reth/db/settings",
                                collapsed: true,
                                items: [
                                    {
                                        text: "op-reth db settings get",
                                        link: "/cli/op-reth/db/settings/get"
                                    },
                                    {
                                        text: "op-reth db settings set",
                                        link: "/cli/op-reth/db/settings/set",
                                        collapsed: true,
                                        items: [
                                            {
                                                text: "op-reth db settings set receipts_in_static_files",
                                                link: "/cli/op-reth/db/settings/set/receipts_in_static_files"
                                            },
                                            {
                                                text: "op-reth db settings set transaction_senders_in_static_files",
                                                link: "/cli/op-reth/db/settings/set/transaction_senders_in_static_files"
                                            }
                                        ]
                                    }
                                ]
                            },
                            {
                                text: "op-reth db static-file-header",
                                link: "/cli/op-reth/db/static-file-header",
                                collapsed: true,
                                items: [
                                    {
                                        text: "op-reth db static-file-header block",
                                        link: "/cli/op-reth/db/static-file-header/block"
                                    },
                                    {
                                        text: "op-reth db static-file-header path",
                                        link: "/cli/op-reth/db/static-file-header/path"
                                    }
                                ]
                            }
                        ]
                    },
                    {
                        text: "op-reth stage",
                        link: "/cli/op-reth/stage",
                        collapsed: true,
                        items: [
                            {
                                text: "op-reth stage run",
                                link: "/cli/op-reth/stage/run"
                            },
                            {
                                text: "op-reth stage drop",
                                link: "/cli/op-reth/stage/drop"
                            },
                            {
                                text: "op-reth stage dump",
                                link: "/cli/op-reth/stage/dump",
                                collapsed: true,
                                items: [
                                    {
                                        text: "op-reth stage dump execution",
                                        link: "/cli/op-reth/stage/dump/execution"
                                    },
                                    {
                                        text: "op-reth stage dump storage-hashing",
                                        link: "/cli/op-reth/stage/dump/storage-hashing"
                                    },
                                    {
                                        text: "op-reth stage dump account-hashing",
                                        link: "/cli/op-reth/stage/dump/account-hashing"
                                    },
                                    {
                                        text: "op-reth stage dump merkle",
                                        link: "/cli/op-reth/stage/dump/merkle"
                                    }
                                ]
                            },
                            {
                                text: "op-reth stage unwind",
                                link: "/cli/op-reth/stage/unwind",
                                collapsed: true,
                                items: [
                                    {
                                        text: "op-reth stage unwind to-block",
                                        link: "/cli/op-reth/stage/unwind/to-block"
                                    },
                                    {
                                        text: "op-reth stage unwind num-blocks",
                                        link: "/cli/op-reth/stage/unwind/num-blocks"
                                    }
                                ]
                            }
                        ]
                    },
                    {
                        text: "op-reth p2p",
                        link: "/cli/op-reth/p2p",
                        collapsed: true,
                        items: [
                            {
                                text: "op-reth p2p header",
                                link: "/cli/op-reth/p2p/header"
                            },
                            {
                                text: "op-reth p2p body",
                                link: "/cli/op-reth/p2p/body"
                            },
                            {
                                text: "op-reth p2p bootnode",
                                link: "/cli/op-reth/p2p/bootnode"
                            },
                            {
                                text: "op-reth p2p rlpx",
                                link: "/cli/op-reth/p2p/rlpx",
                                collapsed: true,
                                items: [
                                    {
                                        text: "op-reth p2p rlpx ping",
                                        link: "/cli/op-reth/p2p/rlpx/ping"
                                    }
                                ]
                            }
                        ]
                    },
                    {
                        text: "op-reth config",
                        link: "/cli/op-reth/config"
                    },
                    {
                        text: "op-reth prune",
                        link: "/cli/op-reth/prune"
                    },
                    {
                        text: "op-reth re-execute",
                        link: "/cli/op-reth/re-execute"
                    }
                ]
            }
        ]
    },
]