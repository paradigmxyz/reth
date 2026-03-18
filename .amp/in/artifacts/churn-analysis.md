# Arena Sparse Trie: Node Churn by Depth

**Goal:** Determine if there exists a node depth below which RLP caching during `update_leaves` is safe — i.e., the cached value is very unlikely to be invalidated by a subsequent update in the same batch.

**Method:** Instrumented `update_leaves` to count, per node, how many updates flow through it. "Churn" = a node touched by >1 update. Node depth is measured by actual trie nodes traversed (not nibble path length), so extension keys are folded in.

**Data source:** reth-bench on dev-brian against mainnet blocks.

---

## Takeaway

| Trie | Safe Caching Depth | Churn Rate | Nodes Covered |
|------|-------------------:|-----------:|--------------:|
| Account | ≥ 4            |      0.03% |        58.27% |
| Storage | ≥ 3            |      0.04% |        61.19% |

At these thresholds, >58% of all nodes could have their RLP eagerly cached during `update_leaves` with <0.05% risk of invalidation. One depth higher (account ≥ 3 / storage ≥ 2) captures 73% / 81% of nodes but at ~0.4-0.5% churn — still potentially acceptable if the cost of a wasted RLP encode is small relative to the savings.

---

## Account Trie

| Depth | Nodes Touched | % of All Nodes | Nodes w/ Churn | Churn Rate | Cumul % (≥ depth) |
|------:|--------------:|---------------:|---------------:|-----------:|------------------:|
|     0 |        86,548 |          0.05% |         86,850 |    100.35% |           100.00% |
|     1 |    17,499,343 |         10.51% |      4,509,882 |     25.77% |            99.95% |
|     2 |    27,030,259 |         16.24% |      1,007,278 |      3.73% |            89.43% |
|     3 |    24,832,579 |         14.92% |        117,352 |      0.47% |            73.19% |
|   **4** | **24,939,492** |     **14.98%** |      **7,935** |  **0.03%** |        **58.27%** |
|     5 |    24,946,277 |         14.99% |            735 |      0.00% |            43.29% |
|     6 |    24,639,385 |         14.80% |             79 |      0.00% |            28.30% |
|     7 |    18,723,767 |         11.25% |              2 |      0.00% |            13.50% |
|     8 |     3,525,613 |          2.12% |              0 |      0.00% |             2.25% |
|     9 |       218,254 |          0.13% |              0 |      0.00% |             0.13% |
|    10 |           383 |          0.00% |              0 |      0.00% |             0.00% |

## Storage Trie

| Depth | Nodes Touched | % of All Nodes | Nodes w/ Churn | Churn Rate | Cumul % (≥ depth) |
|------:|--------------:|---------------:|---------------:|-----------:|------------------:|
|     0 |       559,991 |          0.50% |        457,187 |     81.64% |           100.00% |
|     1 |    20,421,890 |         18.14% |        876,041 |      4.29% |            99.50% |
|     2 |    22,707,925 |         20.17% |         98,876 |      0.44% |            81.36% |
|   **3** | **20,504,036** |     **18.22%** |      **8,881** |  **0.04%** |        **61.19%** |
|     4 |    19,879,444 |         17.66% |            400 |      0.00% |            42.97% |
|     5 |    17,809,665 |         15.82% |             25 |      0.00% |            25.31% |
|     6 |     9,825,453 |          8.73% |              2 |      0.00% |             9.49% |
|     7 |       836,921 |          0.74% |              0 |      0.00% |             0.76% |
|     8 |        21,336 |          0.02% |              0 |      0.00% |             0.02% |
|     9 |            92 |          0.00% |              0 |      0.00% |             0.00% |

