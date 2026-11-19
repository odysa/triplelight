//! Identical harness for baseline and candidate; serial, warm, exact-result checks.
//! Usage: count_probe SNAPSHOT_DIR QUERY_DIR OUTPUT.tsv
use triplelight::{
    exec::{Options, QueryResult, query_with},
    opt::Stats,
    storage::DiskSnapshot,
};
use sha2::{Digest, Sha256};
use std::{error::Error, fs, time::Instant};
fn normalize(result: QueryResult) -> Result<Vec<String>, Box<dyn Error>> {
    let QueryResult::Solutions { vars, rows } = result else {
        return Err("SELECT solutions expected".into());
    };
    let mut out = Vec::new();
    for row in rows {
        let mut cells: Vec<_> = vars
            .iter()
            .zip(row)
            .map(|(v, t)| (v.as_str().to_owned(), t.map(|x| x.to_string())))
            .collect();
        cells.sort_by(|a, b| a.0.cmp(&b.0));
        out.push(format!("{cells:?}"));
    }
    out.sort();
    Ok(out)
}
fn scalar(result: &QueryResult) -> String {
    if let QueryResult::Solutions { vars, rows } = result {
        if vars.len() == 1 && rows.len() == 1 {
            if let Some(oxrdf::Term::Literal(value)) = &rows[0][0] {
                return value.value().to_owned();
            }
        }
    }
    String::new()
}
fn main() -> Result<(), Box<dyn Error>> {
    let a: Vec<_> = std::env::args().collect();
    if a.len() != 4 {
        return Err("usage: count_probe SNAPSHOT_DIR QUERY_DIR OUTPUT.tsv".into());
    }
    let snap = DiskSnapshot::open(&a[1])?;
    let stats = Stats::from_snapshot(&snap)?;
    let options = Options {
        stats: Some(&stats),
        wcoj: true,
        raw_ids: false,
    };
    let mut paths: Vec<_> = fs::read_dir(&a[2])?
        .map(|p| p.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    paths.retain(|p| p.extension().is_some_and(|x| x == "rq"));
    paths.sort();
    let answers = std::path::PathBuf::from(format!("{}.answers", a[3]));
    fs::create_dir_all(&answers)?;
    let mut output = String::from("query\tsample\trows\tsha256\tquery_us\tcount_value\n");
    for path in paths {
        let name = path.file_stem().unwrap().to_string_lossy();
        let text = fs::read_to_string(&path)?;
        let warm = query_with(&snap, &text, &options)?;
        if std::env::var_os("COUNT_PROBE_SAVE_JSON").is_some() {
            fs::write(answers.join(format!("{name}.json")),triplelight::exec::results::serialize(&warm).1)?;
        }
        let expected = normalize(warm)?;
        let hash = format!("{:x}", Sha256::digest(expected.join("\n").as_bytes()));
        if expected.len() <= 32 {
            fs::write(answers.join(format!("{name}.txt")), expected.join("\n"))?;
        }
        for sample in 0..5 {
            let start = Instant::now();
            let result = query_with(&snap, &text, &options)?;
            let us = start.elapsed().as_micros();
            let value = scalar(&result);
            let rows = normalize(result)?;
            if rows != expected {
                return Err(format!("unstable answers for {name}").into());
            }
            use std::fmt::Write;
            writeln!(
                output,
                "{name}\t{sample}\t{}\t{hash}\t{us}\t{value}",
                rows.len()
            )?;
        }
        fs::write(&a[3], &output)?;
        eprintln!("{name}: {} rows, {hash}", expected.len());
    }
    Ok(())
}
