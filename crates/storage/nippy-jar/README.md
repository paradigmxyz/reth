
Storage format to save snapshots on. Trying to be random_read friendly


#####

similar and subject to changes structure:

`.nj`

```bash
--
# Each column has its own compression dictionary
row0: col0 | ... | colN
...
rowN: col0 | ... | colN
--
```

`.idx`

```bash
--
# tx/block hash check
Option<bloomfilter<_>>
--
# tx/block hash -> Index position
Option<PHF<_>>
--
OffsetList [ offset_0, ..., offset_N , ...]
# For TxHash at offset_0, PHF will return a "random" integer. This is an index of an index.
PHFList [ posN, ... , pos0 = OffsetList(0), ... ,posX ]
--
Option<CompressionDictCol0>
...
Option<CompressionDictColN>
--


```


