import { SidebarItem } from "vocs";
import { rethCliSidebar } from "./sidebar-cli-reth";
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
                        text: "Signature Verification",
                        link: "/installation/binaries#signature-verification",
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
            rethCliSidebar
        ]
    },
]