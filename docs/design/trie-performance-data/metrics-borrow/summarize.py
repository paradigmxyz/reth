#!/usr/bin/env python3
"""Summarize the captured ABBA cursor metrics samples, without new measurements."""
import csv
import json
import pathlib
import statistics

root = pathlib.Path(__file__).resolve().parent
fields = ('durable_ns', 'write_ns', 'commit_ns')

def stats(rows):
    result = {}
    for field in fields:
        values = [int(row[field]) for row in rows]
        q1, _, q3 = statistics.quantiles(values, n=4, method='inclusive')
        result[field] = dict(median=statistics.median(values), q1=q1, q3=q3,
                             mean=statistics.mean(values))
    return result

result = dict(row_count=0, runs={}, pooled={}, pairs=[])
pooled = {variant: {mode: [] for mode in ('off', 'on')}
          for variant in ('baseline', 'candidate')}
for path in sorted(root.glob('run-*-*.csv')):
    _, run, variant = path.stem.split('-')
    rows = list(csv.DictReader(path.open()))
    assert len(rows) == 80
    record = result['runs'][run] = dict(variant=variant)
    for mode in ('off', 'on'):
        selected = [row for row in rows if row['metrics'] == mode]
        assert len(selected) == 40
        assert {int(row['cursor_calls']) for row in selected} == ({20480} if mode == 'on' else {0})
        record[mode] = stats(selected)
        pooled[variant][mode].extend(selected)
    result['row_count'] += len(rows)
assert result['row_count'] == 640
result['pooled'] = {variant: {mode: stats(rows) for mode, rows in modes.items()}
                    for variant, modes in pooled.items()}
for baseline, candidate in ((1, 2), (4, 3), (5, 6), (8, 7)):
    before, after = result['runs'][str(baseline)], result['runs'][str(candidate)]
    assert before['variant'] == 'baseline' and after['variant'] == 'candidate'
    pair = dict(baseline_run=baseline, candidate_run=candidate)
    for mode in ('off', 'on'):
        pair[mode] = {field: 100 * (after[mode][field]['median'] / before[mode][field]['median'] - 1)
                      for field in fields}
        pair[mode]['durable_mean_change_percent'] = 100 * (
            after[mode]['durable_ns']['mean'] / before[mode]['durable_ns']['mean'] - 1)
    pair['normalized_write_change_percent'] = 100 * (
        (1 + pair['on']['write_ns'] / 100) / (1 + pair['off']['write_ns'] / 100) - 1)
    result['pairs'].append(pair)
(root / 'summary.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result['pairs'], indent=2))
