---
title: '(Custom EVM tracer): Reported opcode gas cost is incorrect'
labels:
    - A-rpc
    - C-bug
    - M-prevent-stale
assignees:
    - Pana
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.976106Z
info:
    author: raxhvl
    created_at: 2024-09-17T14:11:30Z
    updated_at: 2025-06-11T10:29:04Z
---

### Describe the bug

When using a custom evm tracer, the gas cost for opcoded reported by the tracer is incorrect.

### Steps to reproduce

Consider the following `debug_traceTransaction` RPC reqeust:

```python
import asyncio
import json
import os

from aiohttp import ClientSession, TCPConnector


async def custom_tracer():
    async with ClientSession(connector=TCPConnector(limit=0)) as session:
        method = "debug_traceTransaction"
        tracer = """
        {
            data: [],
            memoryInstructions: {"MSTORE":"W", "MSTORE8":"B", "MLOAD":"R"},
            fault: function (log) {},
            step: function (log) {
                let op = log.op.toString();
                let instructions = this.memoryInstructions
                if (Object.keys(instructions).includes(op)) {
                this.data.push({
                    op: instructions[op] ,
                    depth: log.getDepth(),
                    offset: log.stack.peek(0),
                    gasCost: log.getCost(),
                    memorySize: log.memory.length(),
                });
                }
            },
            result: function (ctx, _) {
                return {
                    error: !!ctx.error,
                    data: this.data
                };
            }
        }
        """
        params = [
            "0x82f6413a2658ebb83f27e44fe1e815ec0979a7dcc0bc9dbdbbbe058d547195a6",
            {"tracer": tracer},
        ]

        payload = {
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1,
        }

        async with session.post(
            "https://lb.nodies.app/v2/8bfbba93396a46b1a3d99311daf0dae6",
            json=payload,
            headers={"x-api-key": "b27881a7-4ea2-439f-a1c6-77ad0c875c16"},
        ) as response:
            result = await response.json()
            if "error" in result:
                raise Exception(f"RPC Error ({method}>req:{params}): {result['error']}")

            with open(os.path.join("trace.json"), "w") as output_file:
                json.dump(result, output_file)
asyncio.run(custom_tracer())
```
Expected gas costs:

```json
[
  6,  6, 3, 3, 12, 3,  3,  9, 6, 3, 3, 3,
  3,  3, 3, 3,  3, 6,  6,  3, 3, 3, 3, 3,
  3,  3, 3, 3,  3, 3,  3,  3, 3, 6, 6, 3,
  3,  3, 3, 6,  6, 3,  3,  3, 3, 3, 3, 6,
  6,  3, 6, 3,  3, 3,  3, 12, 3, 3, 9, 6,
  3, 12, 6, 3,  3, 3,  3,  3, 3, 3, 3, 6,
  3,  6, 3, 6,  3, 3, 12,  3, 3, 3, 3, 3,
  3,  3, 6, 3,  3, 3,  3,  3, 3, 3, 3, 3,
  3,  3, 3, 3,
  ... 5998 more items
]
```

Reported gas cost:
```json
[
    138,   150,   229,   317,     6,  2148,  2163,  2175,  2196,
   2642,  2657,  2925,  2940,  3191,  3206,  4376,  4391,  4403,
   4421,  4746,  4761,  4996,  5011,  5284,  5299,  5875,  5890,
   6643,  6658,  7284,  7299,  7493,  7508,  7520,  7538,  7867,
   7882, 10896, 10911, 10923, 10941, 11270, 11285, 11741, 11756,
  14788, 14803, 14815, 14833, 15142, 15256, 15297, 17115, 17235,
  17273,     6,  4184,  4199,  4208,  4232,  4241,  4265,  4295,
   4304,  4328,  4346,  4361,  4379,  4385,  4391,  4465,  4471,
   4547,  4553,  4714,  4726,  4747,  4783,     6,   415,   430,
    697,   712,  3820,  3835,  6943,  6955,  6977,  8804,  8828,
   8847, 13805, 13839, 13847, 13921, 13960, 13991, 14054, 14063,
  14077,
  ... 5998 more items
]
```


### Node logs

_No response_

### Platform(s)

Linux (x86)

### What version/commit are you on?

reth Version: 1.0.0-dev
Commit SHA: 4fd06bd3b
Build Timestamp: 2024-07-06T20:32:43.565379262Z
Build Features: jemalloc
Build Profile: maxperf


### What database version are you on?

Current database version: 2
Local database is uninitialized


### Which chain / network are you on?

mainnet

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [X] I agree to follow the Code of Conduct
