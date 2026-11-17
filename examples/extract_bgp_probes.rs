//! Export isolated BGPs for exact external reference counts.
//! These probes omit enclosing filters/modifiers by design and are not the full query.
use sha2::{Digest, Sha256};
use spargebra::{Query, SparqlParser, algebra::GraphPattern};
use std::{error::Error, fs, io, path::Path};
fn walk<'a>(
    p: &'a GraphPattern,
    path: String,
    out: &mut Vec<(String, &'a GraphPattern)>,
) -> Result<(), Box<dyn Error>> {
    use GraphPattern::*;
    match p {
        Bgp { .. } => out.push((path, p)),
        Join { left, right }
        | LeftJoin { left, right, .. }
        | Union { left, right }
        | Minus { left, right } => {
            let kind = match p {
                Join { .. } => "join",
                LeftJoin { .. } => "optional",
                Union { .. } => "union",
                _ => "minus",
            };
            walk(left, format!("{path}/{kind}.left"), out)?;
            walk(right, format!("{path}/{kind}.right"), out)?;
        }
        Filter { inner, .. }
        | Extend { inner, .. }
        | OrderBy { inner, .. }
        | Project { inner, .. }
        | Distinct { inner }
        | Reduced { inner }
        | Slice { inner, .. }
        | Group { inner, .. } => {
            let kind = match p {
                Filter { .. } => "filter",
                Extend { .. } => "extend",
                OrderBy { .. } => "order",
                Project { .. } => "project",
                Distinct { .. } => "distinct",
                Reduced { .. } => "reduced",
                Slice { .. } => "slice",
                _ => "group",
            };
            walk(inner, format!("{path}/{kind}"), out)?;
        }
        Values { .. } => {} // no BGP here; path still labels its surrounding context
        Graph { .. } | Service { .. } | Path { .. } => {
            return Err(io::Error::other("GRAPH/SERVICE/property-path probes unsupported").into());
        }
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: extract_bgp_probes QUERY_DIR NEW_OUTPUT_DIR".into());
    }
    let output = Path::new(&args[2]);
    fs::create_dir(output)?; // refuse to mix stale files into a new cohort
    fs::create_dir(output.join("select"))?;
    fs::create_dir(output.join("count"))?;
    let mut files: Vec<_> = fs::read_dir(&args[1])?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    files.retain(|p| p.extension().is_some_and(|x| x == "rq" || x == "sparql"));
    files.sort();
    let mut manifest = String::from("probe\tsource\tsource_sha256\talgebra_path\tpatterns\n");
    for file in files {
        let text = fs::read_to_string(&file)?;
        // Conservatively reject expression subqueries rather than omit their BGPs.
        if text.to_ascii_uppercase().contains("EXISTS") {
            return Err(format!("EXISTS expression not supported: {}", file.display()).into());
        }
        let Query::Select { pattern, .. } = SparqlParser::new().parse_query(&text)? else {
            return Err("only SELECT source queries are supported".into());
        };
        let mut nodes = Vec::new();
        walk(&pattern, "root".into(), &mut nodes)?;
        let source_hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        let name = file.file_stem().unwrap().to_string_lossy();
        for (i, (path, node)) in nodes.into_iter().enumerate() {
            let GraphPattern::Bgp { patterns } = node else {
                unreachable!()
            };
            let probe = format!("{name}__bgp{i:02}");
            fs::write(
                output.join("select").join(format!("{probe}.rq")),
                format!("SELECT * WHERE {{ {node} }}\n"),
            )?;
            fs::write(
                output.join("count").join(format!("{probe}.rq")),
                format!("SELECT (COUNT(*) AS ?__rdfdb_diagnostic_count) WHERE {{ {node} }}\n"),
            )?;
            manifest.push_str(&format!(
                "{probe}\t{name}\t{source_hash}\t{path}\t{}\n",
                patterns.len()
            ));
        }
    }
    fs::write(output.join("manifest.tsv"), manifest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_optional_probes_preserve_local_bgp_not_final_query_semantics() {
        let Query::Select { pattern, .. } = SparqlParser::new().parse_query(
            "SELECT DISTINCT ?s WHERE { ?s <urn:p> ?x OPTIONAL { ?s <urn:q> ?y FILTER(?y > 2) } } LIMIT 1"
        ).unwrap() else { unreachable!() };
        let mut nodes = Vec::new();
        walk(&pattern, "root".into(), &mut nodes).unwrap();
        assert_eq!(nodes.len(), 2);
        assert!(nodes[0].0.contains("optional.left"));
        assert!(nodes[1].0.contains("optional.right"));
        for (_, node) in nodes {
            let text = format!("SELECT * WHERE {{ {node} }}");
            let Query::Select {
                pattern: reparsed, ..
            } = SparqlParser::new().parse_query(&text).unwrap()
            else {
                unreachable!()
            };
            let mut roundtrip = Vec::new();
            walk(&reparsed, "root".into(), &mut roundtrip).unwrap();
            assert_eq!(roundtrip.len(), 1);
            assert_eq!(roundtrip[0].1, node);
        }
    }
    #[test]
    fn graph_scope_is_rejected_instead_of_becoming_default_graph_count() {
        let Query::Select { pattern, .. } = SparqlParser::new()
            .parse_query("SELECT * WHERE { GRAPH <urn:g> { ?s ?p ?o } }")
            .unwrap()
        else {
            unreachable!()
        };
        assert!(walk(&pattern, "root".into(), &mut Vec::new()).is_err());
    }
}
