#!/usr/bin/env python3
"""Run one owned import with sampled disk guards and durable receipts.

Guards reserve 8 GiB headroom against the approved 48 GiB cap and 40 GiB
free-space floor. Sampling is not an instantaneous hard storage quota.
Never removes data or another owner's lock. Interrupted imports are not resumable.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time

GIB=1024**3
LOCK=Path('/tmp/rdf-bench.lock/owner')

def allocated(root):
    # du includes all existing evidence and APFS clones conservatively.
    result=subprocess.run(['du','-sk',str(root)],check=True,capture_output=True,text=True)
    return int(result.stdout.split()[0])*1024

def reason(owner, actual_owner, used, free):
    if owner!=actual_owner:return 'lock ownership changed or became unreadable'
    if used>=40*GIB:return 'scratch reached conservative 40 GiB abort threshold'
    if free<=48*GIB:return 'free disk reached conservative 48 GiB abort threshold'
    return None

def snapshot(root,owner):
    try:actual=LOCK.read_text().strip()
    except OSError:actual=None
    used=allocated(root);free=shutil.disk_usage(root).free
    return dict(time_unix=time.time(),allocated_bytes=used,free_bytes=free,
                abort=reason(owner,actual,used,free))

def stop_owned(process):
    if process.poll() is None:
        os.killpg(process.pid,signal.SIGTERM)
        try:process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid,signal.SIGKILL);process.wait()

def supervise(args):
    root=args.root.resolve();receipt=args.receipt.resolve()
    if not root.is_dir() or not receipt.is_relative_to(root):
        raise ValueError('Receipt must be inside existing owned root')
    if receipt.exists() or receipt.with_suffix('.samples.jsonl').exists():
        raise FileExistsError('Refusing to overwrite supervisor evidence')
    initial=snapshot(root,args.owner)
    if initial['abort']:raise RuntimeError(initial['abort'])
    record=dict(schema='owned-import-supervisor-v1',command=args.command,
                root=str(root),owner=args.owner,start=initial,complete=False,
                approved_cap_bytes=48*GIB,free_floor_bytes=40*GIB,
                sample_seconds=2,guards_are_sampled=True)
    def save():
        temporary=receipt.with_suffix('.tmp');temporary.write_text(json.dumps(record,indent=2))
        temporary.replace(receipt)
    save();process=None
    try:
        with receipt.with_suffix('.log').open('x') as log, receipt.with_suffix('.samples.jsonl').open('x') as samples:
            process=subprocess.Popen(args.command,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
            record['pid']=process.pid;save()
            while True:
                point=snapshot(root,args.owner);samples.write(json.dumps(point)+'\n');samples.flush()
                if point['abort']:
                    record['abort']=point['abort'];stop_owned(process);break
                if process.poll() is not None:break
                time.sleep(2)
            record['exit_code']=process.wait()
            record['complete']=record['exit_code']==0 and not record.get('abort')
    except BaseException as exc:
        record['error']=type(exc).__name__+': '+str(exc)
        if process is not None:stop_owned(process)
        raise
    finally:
        record['end_unix']=time.time();save()
    if not record['complete']:raise RuntimeError('Import failed; inspect durable receipt and log')

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root',type=Path,required=True)
    parser.add_argument('--receipt',type=Path,required=True)
    parser.add_argument('--owner',required=True)
    parser.add_argument('command',nargs=argparse.REMAINDER)
    args=parser.parse_args()
    if args.command[:1]==['--']:args.command=args.command[1:]
    if not args.command:parser.error('import command required after --')
    def interrupted(signum,frame):
        raise KeyboardInterrupt('Supervisor received signal '+str(signum))
    signal.signal(signal.SIGTERM,interrupted)
    supervise(args)
