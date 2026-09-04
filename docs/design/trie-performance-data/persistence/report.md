# Storage trie persistence benchmark: adaptive sequential traversal

Each row reports 100 samples in one independently started process pinned to CPU 0 without concurrent builds. The initial database contains 4,096 branch nodes. Dense cases update 4,096 nodes; sparse cases update 256 nodes, spaced 16 paths apart; append adds 4,096 nodes after the existing paths. Times include transaction open, updates, and durable commit. Every sample checks decoded tables after close/reopen against the original writer. Q1/Q3 are the 25th/75th indexed sorted sample values. Baseline and optimized writers alternate execution order within each process.

| Case | Repeat | Baseline median [Q1, Q3], ms | Optimized median [Q1, Q3], ms | Speedup | Time reduction |
|---|---:|---:|---:|---:|---:|
| legacy/unchanged | 1 | 3.263 [3.257, 3.378] | 0.517 [0.510, 0.524] | 6.311× | 84.2% |
| packed/unchanged | 1 | 2.999 [2.998, 3.000] | 0.507 [0.506, 0.510] | 5.912× | 83.1% |
| legacy/replace | 1 | 3.272 [3.256, 3.382] | 2.819 [2.815, 2.823] | 1.161× | 13.8% |
| packed/replace | 1 | 2.999 [2.999, 3.000] | 2.999 [2.998, 3.000] | 1.000× | 0.0% |
| legacy/resize | 1 | 5.481 [5.401, 5.526] | 4.964 [4.961, 4.968] | 1.104× | 9.4% |
| packed/resize | 1 | 5.092 [5.089, 5.093] | 4.653 [4.530, 5.067] | 1.094× | 8.6% |
| legacy/mixed | 1 | 2.835 [2.821, 3.245] | 2.820 [2.815, 2.823] | 1.005× | 0.5% |
| packed/mixed | 1 | 2.999 [2.998, 3.000] | 2.559 [2.437, 2.561] | 1.172× | 14.7% |
| legacy/sparse_replace | 1 | 1.187 [1.175, 1.210] | 1.192 [1.176, 1.220] | 0.996× | -0.4% |
| packed/sparse_replace | 1 | 1.128 [1.113, 1.174] | 1.128 [1.117, 1.174] | 1.000× | -0.0% |
| legacy/sparse_resize | 1 | 2.425 [1.993, 2.433] | 2.429 [1.997, 2.434] | 0.998× | -0.2% |
| packed/sparse_resize | 1 | 2.176 [2.173, 2.180] | 2.176 [2.173, 2.179] | 1.000× | 0.0% |
| legacy/append | 1 | 2.817 [2.806, 2.822] | 2.369 [2.258, 2.384] | 1.189× | 15.9% |
| packed/append | 1 | 2.528 [2.437, 2.561] | 1.999 [1.997, 2.000] | 1.265× | 20.9% |
| legacy/unchanged | 2 | 3.261 [3.256, 3.380] | 0.536 [0.531, 0.547] | 6.079× | 83.5% |
| packed/unchanged | 2 | 3.000 [2.999, 3.006] | 0.532 [0.530, 0.536] | 5.641× | 82.3% |
| legacy/replace | 2 | 3.354 [3.257, 3.382] | 2.820 [2.816, 2.824] | 1.189× | 15.9% |
| packed/replace | 2 | 2.999 [2.998, 3.006] | 2.999 [2.998, 3.000] | 1.000× | 0.0% |
| legacy/resize | 2 | 5.511 [5.401, 5.526] | 4.964 [4.961, 5.392] | 1.110× | 9.9% |
| packed/resize | 2 | 5.091 [5.090, 5.093] | 5.090 [4.654, 5.093] | 1.000× | 0.0% |
| legacy/mixed | 2 | 3.252 [2.856, 3.265] | 2.817 [2.814, 2.820] | 1.154× | 13.4% |
| packed/mixed | 2 | 2.999 [2.998, 3.000] | 2.560 [2.437, 2.562] | 1.172× | 14.6% |
| legacy/sparse_replace | 2 | 1.185 [1.172, 1.198] | 1.195 [1.181, 1.216] | 0.991× | -0.9% |
| packed/sparse_replace | 2 | 1.115 [1.099, 1.159] | 1.123 [1.103, 1.171] | 0.993× | -0.7% |
| legacy/sparse_resize | 2 | 2.415 [1.993, 2.432] | 2.427 [1.994, 2.433] | 0.995× | -0.5% |
| packed/sparse_resize | 2 | 2.175 [2.172, 2.179] | 2.176 [2.173, 2.178] | 1.000× | -0.0% |
| legacy/append | 2 | 2.818 [2.816, 2.821] | 2.358 [2.255, 2.381] | 1.195× | 16.3% |
| packed/append | 2 | 2.438 [2.436, 2.557] | 1.999 [1.998, 2.006] | 1.219× | 18.0% |
| legacy/unchanged | 3 | 3.261 [3.255, 3.380] | 0.528 [0.520, 0.545] | 6.173× | 83.8% |
| packed/unchanged | 3 | 2.999 [2.998, 3.000] | 0.526 [0.525, 0.529] | 5.699× | 82.5% |
| legacy/replace | 3 | 3.377 [3.256, 3.383] | 2.817 [2.814, 2.821] | 1.199× | 16.6% |
| packed/replace | 3 | 2.999 [2.998, 3.000] | 2.999 [2.998, 3.000] | 1.000× | 0.0% |
| legacy/resize | 3 | 5.517 [5.400, 5.528] | 4.964 [4.961, 4.967] | 1.111× | 10.0% |
| packed/resize | 3 | 5.049 [5.046, 5.053] | 4.613 [4.486, 5.049] | 1.094× | 8.6% |
| legacy/mixed | 3 | 2.931 [2.929, 3.253] | 2.813 [2.460, 2.825] | 1.042× | 4.0% |
| packed/mixed | 3 | 3.000 [2.999, 3.000] | 2.534 [2.436, 2.562] | 1.184× | 15.5% |
| legacy/sparse_replace | 3 | 1.178 [1.166, 1.203] | 1.178 [1.165, 1.198] | 1.000× | 0.0% |
| packed/sparse_replace | 3 | 1.121 [1.099, 1.166] | 1.127 [1.106, 1.175] | 0.995× | -0.5% |
| legacy/sparse_resize | 3 | 2.017 [1.996, 2.432] | 2.413 [1.995, 2.430] | 0.836× | -19.6% |
| packed/sparse_resize | 3 | 2.175 [2.173, 2.179] | 2.176 [2.173, 2.181] | 1.000× | -0.0% |
| legacy/append | 3 | 2.819 [2.815, 2.822] | 2.349 [2.254, 2.382] | 1.200× | 16.7% |
| packed/append | 3 | 2.443 [2.434, 2.563] | 1.999 [1.998, 2.003] | 1.222× | 18.2% |

Across repeats (median of three per-process medians; samples are not pooled):

| Case | Baseline, ms | Optimized, ms | Speedup | Time reduction | Repeat speedup range |
|---|---:|---:|---:|---:|---:|
| legacy/unchanged | 3.261 | 0.528 | 6.173× | 83.8% | 6.079–6.311× |
| packed/unchanged | 2.999 | 0.526 | 5.699× | 82.5% | 5.641–5.912× |
| legacy/replace | 3.354 | 2.819 | 1.190× | 15.9% | 1.161–1.199× |
| packed/replace | 2.999 | 2.999 | 1.000× | 0.0% | 1.000–1.000× |
| legacy/resize | 5.511 | 4.964 | 1.110× | 9.9% | 1.104–1.111× |
| packed/resize | 5.091 | 4.653 | 1.094× | 8.6% | 1.000–1.094× |
| legacy/mixed | 2.931 | 2.817 | 1.040× | 3.9% | 1.005–1.154× |
| packed/mixed | 2.999 | 2.559 | 1.172× | 14.7% | 1.172–1.184× |
| legacy/sparse_replace | 1.185 | 1.192 | 0.994× | -0.6% | 0.991–1.000× |
| packed/sparse_replace | 1.121 | 1.127 | 0.995× | -0.5% | 0.993–1.000× |
| legacy/sparse_resize | 2.415 | 2.427 | 0.995× | -0.5% | 0.836–0.998× |
| packed/sparse_resize | 2.175 | 2.176 | 1.000× | -0.0% | 1.000–1.000× |
| legacy/append | 2.818 | 2.358 | 1.195× | 16.3% | 1.189–1.200× |
| packed/append | 2.443 | 1.999 | 1.222× | 18.2% | 1.219–1.265× |
