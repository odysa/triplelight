use triplelight::storage::{MemorySnapshot, Snapshot};
use triplelight::model::Perm;
use oxrdf::{NamedNode, Literal, Triple};
use std::hint::black_box;
fn main() {
    let p = NamedNode::new("urn:p").unwrap();
    let data = (0..100_000).map(|i| Triple::new(NamedNode::new(format!("urn:s{i}")).unwrap(), p.clone(), Literal::new_simple_literal(format!("{i}"))));
    let store = MemorySnapshot::from_triples(data);
    for _ in 0..5 {
        let t=std::time::Instant::now(); let mut total=0u64;
        for i in 0..500_000 { total=total.wrapping_add(black_box(&store).count(black_box(Perm::ALL[i%6]),black_box(&[])).unwrap()); }
        println!("{} {}", t.elapsed().as_nanos(),black_box(total));
    }
}
