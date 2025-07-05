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
                link: "/sdk/overview"
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
                                text: "reth db version",
                                link: "/cli/reth/db/version"
                            },
                            {
                                text: "reth db path",
                                link: "/cli/reth/db/path"
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
                            }
                        ]
                    },
                    {
                        text: "reth config",
                        link: "/cli/reth/config"
                    },
                    {
                        text: "reth debug",
                        link: "/cli/reth/debug",
                        collapsed: true,
                        items: [
                            {
                                text: "reth debug execution",
                                link: "/cli/reth/debug/execution"
                            },
                            {
                                text: "reth debug merkle",
                                link: "/cli/reth/debug/merkle"
                            },
                            {
                                text: "reth debug in-memory-merkle",
                                link: "/cli/reth/debug/in-memory-merkle"
                            },
                            {
                                text: "reth debug build-block",
                                link: "/cli/reth/debug/build-block"
                            }
                        ]
                    },
                    {
                        text: "reth recover",
                        link: "/cli/reth/recover",
                        collapsed: true,
                        items: [
                            {
                                text: "reth recover storage-tries",
                                link: "/cli/reth/recover/storage-tries"
                            }
                        ]
                    },
                    {
                        text: "reth prune",
                        link: "/cli/reth/prune"
                    }
                ]
            }
        ]
    },
]