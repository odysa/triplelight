//! Parses SPARQL files with TripleLight's own parser and prints one line per file: `ok <file>` or `err <file> <msg>`.
//! Usage: sparql_parse (query|update) <base-iri-prefix> <file>...  (each file's own path is used as its base IRI).
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let kind = &args[1];
    for f in &args[2..] {
        let text = std::fs::read_to_string(f).unwrap_or_default();
        let base = format!("file://{f}");
        let r = if kind == "update" {
            triplelight::sparql::parse_update(&text, Some(&base)).map(|_| ())
        } else {
            triplelight::sparql::parse_query(&text, Some(&base)).map(|_| ())
        };
        match r { Ok(()) => println!("ok {f}"), Err(e) => println!("err {f} {}", e.to_string().replace('\n', " ")) }
    }
}
