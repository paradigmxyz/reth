#!/usr/bin/env python3
"""Run matched mincore benchmark binaries sequentially in ABBA order."""
import argparse,csv,datetime,hashlib,io,json,os,platform,statistics,subprocess
from pathlib import Path
p=argparse.ArgumentParser()
p.add_argument('baseline',type=Path)
p.add_argument('candidate',type=Path)
p.add_argument('--output',type=Path,required=True)
p.add_argument('--samples',type=int,default=12)
p.add_argument('--rounds',type=int,default=2)
p.add_argument('--cpu',type=int,default=1)
p.add_argument('--filter',default='')
a=p.parse_args()
a.output.mkdir(parents=True,exist_ok=False)
(a.output/'runs').mkdir()
bins={k:getattr(a,k).resolve() for k in ['baseline','candidate']}
meta={'started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'cpu_affinity':[a.cpu],'samples':a.samples,'rounds':a.rounds,'filter':a.filter,'order':['baseline','candidate','candidate','baseline'],'platform':platform.platform(),'binaries':{k:{'path':str(v),'sha256':hashlib.sha256(v.read_bytes()).hexdigest()} for k,v in bins.items()},'statistics':'median and inclusive quartiles of process-level medians; not confidence intervals'}
(a.output/'metadata.json').write_text(json.dumps(meta,indent=2)+'\n')
records=[]
for rnd in range(1,a.rounds+1):
 for pos,mode in enumerate(meta['order'],1):
  name=f'round-{rnd}-run-{pos}-{mode}'
  print(name,flush=True)
  env=os.environ.copy();env.update(MPT_MINCORE_SAMPLES=str(a.samples),MPT_MINCORE_FILTER=a.filter)
  r=subprocess.run(['taskset','-c',str(a.cpu),str(bins[mode])],env=env,text=True,capture_output=True)
  (a.output/'runs'/f'{name}.csv').write_text(r.stdout)
  (a.output/'runs'/f'{name}.stderr').write_text(r.stderr)
  if r.returncode: raise RuntimeError(f'{name} failed: {r.stderr}')
  expected_macro=1 if mode=='baseline' else 0
  assert f'MDBX_USE_MINCORE={expected_macro} ' in r.stderr
  rows=list(csv.DictReader(io.StringIO(r.stdout)))
  assert rows
  for row in rows:
   assert int(row['mincore'])==expected_macro
   if expected_macro==0: assert int(row['mincore_calls'])==0
   else: assert int(row['mincore_calls'])>0
   records.append(dict(row,mode=mode,round=rnd,position=pos))
with (a.output/'runs.csv').open('w') as f:
 w=csv.DictWriter(f,fieldnames=list(records[0]));w.writeheader();w.writerows(records)
processes={}
for row in records:
 key=(row['mode'],row['round'],row['position'],row['case'])
 processes.setdefault(key,[]).append(row)
proc=[]
for (mode,rnd,pos,case),rows in processes.items():
 assert len(rows)==a.samples
 out={'mode':mode,'round':rnd,'position':pos,'case':case}
 for k in rows[0]:
  if k not in ['mode','round','position','case','sample']:
   out[k]=statistics.median(int(r[k]) for r in rows)
 proc.append(out)
with (a.output/'process-medians.csv').open('w') as f:
 w=csv.DictWriter(f,fieldnames=list(proc[0]));w.writeheader();w.writerows(proc)
summary=[]
for case in sorted(set(r['case'] for r in proc)):
 for field in [k for k in proc[0] if k not in ['mode','round','position','case','mincore']]:
  vals={m:[r[field] for r in proc if r['case']==case and r['mode']==m] for m in bins}
  out={'case':case,'field':field,'runs_per_mode':len(vals['baseline'])}
  for mode,values in vals.items():
   lower,_,upper=statistics.quantiles(values,n=4,method='inclusive')
   out.update({mode+'_median':statistics.median(values),mode+'_p25':lower,mode+'_p75':upper})
  b=out['baseline_median'];c=out['candidate_median']
  out['candidate_change_percent']=100*(c/b-1) if b else ''
  summary.append(out)
with (a.output/'summary.csv').open('w') as f:
 w=csv.DictWriter(f,fieldnames=list(summary[0]));w.writeheader();w.writerows(summary)
meta['finished_utc']=datetime.datetime.now(datetime.timezone.utc).isoformat()
(a.output/'metadata.json').write_text(json.dumps(meta,indent=2)+'\n')
print(a.output/'summary.csv',flush=True)
