#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use triplelight::storage::{DiskBase, Snapshot};
use triplelight::store::{Store, StoreUpdate};

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn text_index_preserves_store_bytes_and_sorts_unique_postings() {
    let dir = Fixture(std::env::temp_dir().join(format!("tl-text-index-{}", std::process::id())));
    fs::create_dir(&dir.0).unwrap();
    let nt = dir.0.join("seed.nt");
    fs::write(&nt, concat!(
        "<urn:a> <urn:label> \"Graph graph Straße\"@de .\n",
        "<urn:b> <urn:label> \"graph\" .\n",
        "<urn:c> <urn:label> \"42\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n",
    )).unwrap();
    let root = dir.0.join("database");
    let store = Store::open_with(&root, Some(&nt), Box::new(DiskBase { workers: 1 })).unwrap();
    let query = "PREFIX tl: <urn:triplelight:text:> SELECT ?s ?label WHERE { ?s <urn:label> ?label . FILTER(tl:matches(?label, \"STRASSE graph\")) } ORDER BY ?s";
    let answer = |store: &Store| {
        let snapshot = store.snapshot();
        let result = triplelight::exec::query(&*snapshot, query).unwrap();
        triplelight::exec::results::serialize(&result).1
    };
    let baseline = answer(&store);
    let current_before = fs::read(root.join("CURRENT")).unwrap();
    let manifest_before = fs::read(root.join("base-0/MANIFEST")).unwrap();
    let wal_before = fs::read(root.join("wal.log")).unwrap();
    assert_eq!(store.build_text_index(2).unwrap(), 2);
    assert_eq!(fs::read(root.join("CURRENT")).unwrap(), current_before);
    assert_eq!(fs::read(root.join("base-0/MANIFEST")).unwrap(), manifest_before);
    assert_eq!(fs::read(root.join("wal.log")).unwrap(), wal_before);

    let file = fs::read(root.join("text-0.v1")).unwrap();
    let word = |offset: usize| u64::from_le_bytes(file[offset..offset + 8].try_into().unwrap()) as usize;
    assert_eq!(&file[..8], b"TLTEXT01");
    assert_eq!(word(8), 0);
    assert_eq!(word(16), 1);
    assert_eq!(word(72), 2);
    let directory = 112 + word(56);
    let postings2 = word(directory + 21);
    assert!(postings2 > 0 && postings2 < word(56));
    assert_eq!(word(directory + 8), 2);
    assert_eq!(&file[directory + 16..directory + 21], b"graph");
    assert_eq!(word(directory + 29), 1);
    assert_eq!(&file[directory + 37..directory + word(64)], b"strasse");
    assert_eq!(file.len(), directory + word(64) + 16);
    drop(store);

    let reopen = || Store::open_with(&root, None, Box::new(DiskBase { workers: 1 })).unwrap();
    let store = reopen();
    let pinned = store.snapshot();
    assert_eq!(pinned.text_candidates(&["graph", "strasse"], 8).unwrap().len(), 1);
    assert_eq!(pinned.text_candidates(&["graph"], 8).unwrap().len(), 2);
    assert!(pinned.text_candidates(&["graph"], 1).is_none());
    assert_eq!(answer(&store), baseline);

    drop(store);
    let mut corrupt = file.clone();
    corrupt[112] ^= 1;
    let temp = root.join("index-replacement");
    let index = root.join("text-0.v1");
    fs::write(&temp, corrupt).unwrap();
    fs::rename(&temp, &index).unwrap();
    let store = reopen();
    assert!(store.snapshot().text_candidates(&["graph"], 8).is_none());
    assert_eq!(answer(&store), baseline);
    assert_eq!(pinned.text_candidates(&["graph"], 8).unwrap().len(), 2);
    drop(store);

    fs::write(&temp, file).unwrap();
    fs::rename(temp, index).unwrap();
    let store = reopen();
    store.update("INSERT DATA { <urn:new> <urn:label> \"Graph Straße\" }").unwrap();
    assert!(store.snapshot().text_candidates(&["graph"], 8).is_none());
    let updated = answer(&store);
    assert!(updated.contains("urn:new"));
    store.compact().unwrap();
    assert!(!root.join("text-0.v1").exists());
    assert_eq!(pinned.text_candidates(&["graph"], 8).unwrap().len(), 2);
    assert!(store.snapshot().text_candidates(&["graph"], 8).is_none());
    assert_eq!(answer(&store), updated);
    assert_eq!(store.build_text_index(2).unwrap(), 2);
    drop(store);

    fs::write(root.join("text-0.v1"), "abandoned").unwrap();
    let store = reopen();
    assert!(!root.join("text-0.v1").exists());
    assert_eq!(store.snapshot().text_candidates(&["graph"], 8).unwrap().len(), 3);
    assert_eq!(pinned.text_candidates(&["graph"], 8).unwrap().len(), 2);
    assert_eq!(answer(&store), updated);
}
