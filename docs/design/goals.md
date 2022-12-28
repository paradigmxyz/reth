# Reth Goals

### Why are we building this client?

Our goal in building Reth, apart from improving client diversity, is to create a client that delivers maximally along each of the following dimensions:

- Performance
- Configurability
- Open-source friendliness

---

## Performance

### Why does performance matter?

This is a win for everyone:
- Average users & developers benefit from RPC performance, leading to more responsive applications and faster feedback.
- Home node operators benefit from faster sync times.
- Costs are lowered for all operators, whether in terms of storage costs, or being able to serve more requests from the same node.
- Searchers are able to run more simulations.

### What are the performance bottlenecks that need to be addressed?

**Optimizing state access**

The pipeline that a given transaction goes through as it’s processed is more or less the following:

RPC -> EVM -> Cache -> Codec -> DB

One of our first and foremost goals in Reth is to minimize the latency and maximize the throughput (think: request concurrency) of this pipeline.

Why? This is a win for everyone. RPC providers meet more impressive SLAs, MEV searchers become more effective, home nodes sync faster, etc.

The biggest bottleneck in this pipeline is not the execution of the EVM interpreter itself, but rather in accessing state and managing I/O. As such, we think the largest optimizations to be made are closest to the DB layer.

Ideally, we can achieve such fast runtime operation that we can avoid storing certain things (e.g.?) on the disk, and are able to generate them on the fly, instead - minimizing disk footprint.

---

## Configurability

### Why does configurability matter?

**Control over tradeoffs**

Almost any given design choice or optimization to the client comes with its own tradeoffs. As such, our long-term goal is not to make opinionated decisions on behalf of everyone, as some users will be negatively impacted and turned away from what could be a great client.

**Profiles**

We aim to facilitate the creation of community-developed configuration presets that are fit to various user profiles, e.g. archive node, RPC provider, MEV searcher, etc.

**Extension to EVM-compatible L1s and L2s**

Another consequence of a configurable design is the ability to quickly extend the client to support other EVM-compatible L1s and L2s, enabling innovation while retaining performance.

### How is Reth made configurable?

**Modularity & generics**

We prioritize a modular design for Reth with reasonable (and zero-cost!) abstractions over generic interfaces. We want it to be quick and easy for others to extend or adapt the implementation to their own needs.

---

## Open-source friendliness

### Why does open-source friendliness matter?

Maintaining a client implementation is *hard*. Bringing in talent and sustaining momentum in workstreams is a known challenge. As such, we take an open-source first approach to ensure that the development of Reth can be carried forward by the community.

We want to be as deliberate as possible in forming a feedback loop with the Ethereum community, and not only make it easy to contribute to Reth, but in fact actively *encourage* doing so.

Our goal is that community members with no Rust experience, and no experience running a node, will still be able to meaningfully contribute to the project, and accrue expertise in doing so.

### How does Reth support open-source contribution?

**Documentation**

It goes without saying that verbose and thorough documentation is a must. The docs should provide full context on the design and implementation of the client, as well as the contribution process, and should be accessible to anyone with a basic understanding of Ethereum.

**Issue tracking**

Everything that is (and is not) being worked on within the client should be tracked accordingly so that anyone in the community can stay on top of the state of development. This makes it clear what kind of help is needed, and where.