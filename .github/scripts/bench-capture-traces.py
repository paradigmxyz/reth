import gzip
import json
import pathlib
import sys
import urllib.request

meta = json.loads((pathlib.Path(sys.argv[1]) / 'corpus.meta.json').read_text())
blocks = sorted(set(range(meta['corpus_from'], meta['corpus_to'] + 1)) | {25983515, 25983519, 25983521, 25983528})
output = pathlib.Path(sys.argv[2]) / 'trace-diagnostics'
output.mkdir(exist_ok=True)
for number in blocks:
    for method, params in [
        ('debug_traceBlockByNumber', [hex(number), {'tracer': 'callTracer'}]),
        ('trace_block', [hex(number)]),
        ('trace_replayBlockTransactions', [hex(number), ['trace', 'stateDiff']]),
    ]:
        request = urllib.request.Request('http://127.0.0.1:8545', data=json.dumps({'jsonrpc':'2.0','id':1,'method':method,'params':params}).encode(), headers={'Content-Type':'application/json'})
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                data = response.read()
            with gzip.open(output / f'{number}-{method}.json.gz', 'wb') as f:
                f.write(data)
        except Exception as error:
            print(number, method, str(error), flush=True)
