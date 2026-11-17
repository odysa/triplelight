#!/usr/bin/env python3
"""Export deterministic official BI parameters; no endpoint or query execution."""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream,'sha256').hexdigest()


def recover_parameters(template, query):
    """Recover exact replacement strings; repeated placeholders must agree."""
    pieces = []; seen = set(); start = 0
    for match in re.finditer(r'%([A-Za-z0-9_]+)%', template):
        pieces.append(re.escape(template[start:match.start()]))
        name = match[1]
        pieces.append('(?P='+name+')' if name in seen else '(?P<'+name+'>.*?)')
        seen.add(name); start = match.end()
    pieces.append(re.escape(template[start:]))
    match = re.fullmatch(''.join(pieces), query, re.S)
    if match is None:
        raise ValueError('Export differs from template substitution')
    result = match.groupdict()
    assert re.sub(r'%([A-Za-z0-9_]+)%', lambda m: result[m[1]], template) == query
    return result


def make(args):
    if Path('/tmp/rdf-bench.lock/owner').read_text().strip() != args.lock_owner:
        raise RuntimeError('Own the shared benchmark lock before Java compilation/export')
    args.out.mkdir(parents=True,exist_ok=False)
    base = Path(__file__).resolve().parent
    templates = base/'bsbm-bi/templates'
    classes = args.out/'classes'; classes.mkdir()
    classpath = os.pathsep.join([str(args.tools.resolve()),str(args.tools.resolve()/'lib/*')])
    with (args.out/'generator.log').open('wb') as log:
        subprocess.run(['javac','-cp',classpath,'-d',str(classes),str(base/'ExportBiQueries.java')],
                       stdout=log,stderr=subprocess.STDOUT,check=True)
        subprocess.run(['java','-cp',str(classes)+os.pathsep+classpath,'ExportBiQueries',str(templates),
                        str(args.resources.resolve()),str(args.out/'queries'),str(args.seed),str(args.mixes),str(args.warmups)],
                       stdout=log,stderr=subprocess.STDOUT,check=True)
    mix = [int(v) for v in (templates/'querymix.txt').read_text().split()]
    manifest = {'schema':'bsbm-bi-export-v1','seed':args.seed,'mixes':args.mixes,'warmup_mixes':args.warmups,
                'official_default_warmup_mixes':50,'official_default_measured_mixes':500,
                'scale':args.scale,'weighted_mix':mix,'source_data_sha256':args.data_sha256,
                'tools':str(args.tools.resolve()),'resources':str(args.resources.resolve()),
                'resource_hashes':{p.name:sha(p) for p in sorted(args.resources.iterdir()) if p.is_file()},
                'jar_hashes':{p.name:sha(p) for p in sorted((args.tools/'lib').glob('*.jar'))},'queries':[]}
    with (args.out/'queries/index.tsv').open() as stream:
        for row in csv.DictReader(stream,delimiter='\t'):
            path = args.out/'queries'/row['file']; number = int(row['template'])
            text = path.read_text()
            manifest['queries'].append({'file':row['file'],'mix':int(row['mix']), 'position':int(row['position']),
                 'template':number,'warmup':row['warmup']=='true','sha256':sha(path),
                 'parameters':recover_parameters((templates/f'query{number}.txt').read_text(),text)})
    for m in range(args.mixes):
        assert [q['template'] for q in manifest['queries'] if q['mix']==m] == mix
    (args.out/'manifest.json').write_text(json.dumps(manifest,indent=2))
    print('exported',len(manifest['queries']),'queries')


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    for name in ('tools','resources','out'):p.add_argument('--'+name,type=Path,required=True)
    p.add_argument('--seed',type=int,default=808080)
    p.add_argument('--mixes',type=int,default=4)
    p.add_argument('--warmups',type=int,default=1)
    p.add_argument('--scale',required=True)
    p.add_argument('--data-sha256',required=True)
    p.add_argument('--lock-owner',required=True)
    args=p.parse_args()
    if args.warmups<0 or args.mixes<=args.warmups:p.error('need nonnegative warmups and at least one measured mix')
    make(args)
