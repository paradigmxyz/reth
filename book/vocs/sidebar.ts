import { SidebarItem } from "vocs";

export const sidebar: SidebarItem[] = [
    {
        text: "Introduction",
        items: [
            {
                text: "Overview",
                link: "/overview"
            }
        ]
    },
    {
        text: "Installation",
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
        text: "Run a Node",
        link: "/run/run-a-node",
        items: [
            {
                text: "Mainnet or official testnets",
                link: "/run/mainnet"
            },
            {
                text: "OP Stack",
                link: "/run/optimism"
            },
            {
                text: "Run an OP Mainnet Node",
                link: "/run/sync-op-mainnet"
            },
            {
                text: "Private testnet",
                link: "/run/private-testnet"
            },
            {
                text: "Metrics",
                link: "/run/observability"
            },
            {
                text: "Configuring Reth",
                link: "/run/config"
            },
            {
                text: "Transaction types",
                link: "/run/transactions"
            },
            {
                text: "Pruning & Full Node",
                link: "/run/pruning"
            },
            {
                text: "Ports",
                link: "/run/ports"
            },
            {
                text: "Troubleshooting",
                link: "/run/troubleshooting"
            }
        ]
    },
    {
        text: "Interacting with Reth over JSON-RPC",
        link: "/jsonrpc/intro",
        items: [
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
        items: [
            {
                text: "reth",
                link: "/cli/reth",
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
    {
        text: "Developers",
        link: "/developers/developers",
        items: [
            {
                text: "Execution Extensions",
                
                items: [
                    {
                        text: "Overview",
                        link: "/developers/exex/overview",
                    },
                    {
                        text: "How do ExExes work?",
                        link: "/developers/exex/how-it-works"
                    },
                    {
                        text: "Hello World",
                        link: "/developers/exex/hello-world"
                    },
                    {
                        text: "Tracking State",
                        link: "/developers/exex/tracking-state"
                    },
                    {
                        text: "Remote",
                        link: "/developers/exex/remote"
                    }
                ]
            },
            {
                text: "Contribute",
                link: "/developers/contribute"
            }
        ]
    }
]