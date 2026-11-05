#!/usr/bin/env -S uv run --no-project python
"""Sample node RSS and faults without sampling profiler processes."""
import csv
import json
from pathlib import Path
import sys
import time

output = Path(sys.argv[1])
node_name = sys.argv[2]
output.mkdir(parents=True, exist_ok=True)
seen = set()
with (output / 'memory.csv').open('w') as stream:
    writer = csv.writer(stream)
    writer.writerow(['unix_ms', 'pid', 'start_ticks', 'rss_kib', 'hwm_kib', 'minor_faults', 'major_faults'])
    deadline = time.monotonic() + 14400
    while time.monotonic() < deadline and not (output / 'stop').exists():
        for entry in Path('/proc').iterdir():
            if not entry.name.isdecimal():
                continue
            try:
                if (entry / 'comm').read_text().strip() != node_name:
                    continue
                args = (entry / 'cmdline').read_bytes().split(b'\0')
                if b'node' not in args:
                    continue
                stat = (entry / 'stat').read_text().rsplit(')', 1)[1].split()
                status = dict(line.split(':', 1) for line in (entry / 'status').read_text().splitlines() if ':' in line)
                key = (int(entry.name), int(stat[19]))
                if key not in seen:
                    seen.add(key)
                    with (output / 'processes.jsonl').open('a') as meta:
                        meta.write(json.dumps({'pid':key[0], 'start_ticks':key[1], 'binary':args[0].decode(errors='replace'), 'datadir':args[args.index(b'--datadir')+1].decode(errors='replace') if b'--datadir' in args else None})+'\n')
                writer.writerow([time.time_ns() // 1_000_000, *key, int(status['VmRSS'].split()[0]), int(status['VmHWM'].split()[0]), int(stat[7]), int(stat[9])])
            except (FileNotFoundError, ProcessLookupError, KeyError):
                continue
        stream.flush()
        time.sleep(1)
