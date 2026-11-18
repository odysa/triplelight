//! Fresh-process load comparison: parallel_load <serial|parallel> <file.nt> [workers].
use rdfdb::model::Perm;
use rdfdb::storage::{MemorySnapshot, Snapshot};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 3 { return Err("usage: parallel_load <serial|parallel> <file.nt> [workers]".into()); }
    let started = std::time::Instant::now();
    let db = match args[1].as_str() {
        "serial" => MemorySnapshot::from_ntriples(std::io::BufReader::new(std::fs::File::open(&args[2])?))?,
        "parallel" => MemorySnapshot::from_ntriples_parallel_file(&args[2], args.get(3).map_or(Ok(4), |x| x.parse::<usize>())?)?,
        _ => return Err("expected serial or parallel".into()),
    };
    let elapsed = started.elapsed();
    println!("mode={} triples={} elapsed_seconds={:.6}",args[1],db.count(Perm::Spo,&[])?,elapsed.as_secs_f64());
    Ok(())
}
