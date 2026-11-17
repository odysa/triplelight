#!/usr/bin/env python3
"""Own-process supervisor for the memory-only payoff bound. Caller owns benchmark lock."""
import datetime, hashlib, json, os, pathlib, signal, subprocess, sys, time
binary, dataset, out = map(pathlib.Path,sys.argv[1:4])
queries=list(map(pathlib.Path,sys.argv[4:]))
if not queries: raise SystemExit("usage: run_forced_orders.py BINARY DATA.nt NEW_DIR Q2.rq Q9.rq")
out.mkdir()
def sha(p):
    with p.open("rb") as f:return hashlib.file_digest(f,"sha256").hexdigest()
def receipt(p):
    st=p.stat()
    return {"path":str(p.resolve()),"bytes":st.st_size,"mtime_ns":st.st_mtime_ns,"sha256":sha(p)}
before=receipt(dataset)
meta={"started_utc":datetime.datetime.now(datetime.timezone.utc).isoformat(),
 "binary":receipt(binary),"dataset_before":before,"queries":[receipt(p) for p in queries],
 "kernel_source":receipt(pathlib.Path("src/exec/bgp.rs")),
 "helper_source":receipt(pathlib.Path("examples")/(binary.name+".rs")),
 "execution_sources":{str(p):receipt(p) for p in pathlib.Path("src/exec").glob("*.rs")},
 "git_head":subprocess.check_output(["git","rev-parse","HEAD"],text=True).strip(),
 "uptime_before":subprocess.check_output(["uptime"],text=True).strip(),"timeout_s":900,
 "environment_overrides":{"RDFDB_PROFILE_PLAN":"unset","RDFDB_TRACE_BLOCK_CACHE":"unset","RAYON_NUM_THREADS":str(os.cpu_count() or 1)}}
(out/"receipt.json").write_text(json.dumps(meta,indent=2)+"\n")
env=os.environ.copy()
env.pop("RDFDB_PROFILE_PLAN",None)
env.pop("RDFDB_TRACE_BLOCK_CACHE",None)
env["RAYON_NUM_THREADS"]=str(os.cpu_count() or 1)
with (out/"stdout.log").open("w") as stdout,(out/"resources.log").open("w") as stderr:
    p=subprocess.Popen(["/usr/bin/time","-l",str(binary.resolve()),str(dataset.resolve()),
       str((out/"raw").resolve()),*[str(q.resolve()) for q in queries]],
       stdout=stdout,stderr=stderr,env=env,start_new_session=True)
    (out/"pid").write_text(str(p.pid)+"\n")
    try:
        code=p.wait(timeout=900)
        meta["status"]="complete" if code==0 else "failed"
        meta["exit_code"]=code
    except subprocess.TimeoutExpired:
        os.killpg(p.pid,signal.SIGTERM)
        try:p.wait(timeout=20)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid,signal.SIGKILL);p.wait()
        meta["status"]="timeout";meta["exit_code"]=p.returncode
meta["dataset_after"]=receipt(dataset)
meta["input_unchanged"]=meta["dataset_before"]==meta["dataset_after"]
meta["binary_unchanged"]=meta["binary"]==receipt(binary)
meta["uptime_after"]=subprocess.check_output(["uptime"],text=True).strip()
meta["ended_utc"]=datetime.datetime.now(datetime.timezone.utc).isoformat()
(out/"receipt.json").write_text(json.dumps(meta,indent=2)+"\n")
print(json.dumps({k:meta[k] for k in ["status","exit_code","input_unchanged","binary_unchanged"]}),flush=True)
if meta["status"]!="complete" or not meta["input_unchanged"] or not meta["binary_unchanged"]:
    raise SystemExit(1)
