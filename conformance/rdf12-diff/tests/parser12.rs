use triplelight_core::ntriples::{read_nquads, read_ntriples};
use triplelight_core::turtle::{read_trig, read_turtle};

fn sorted<T: ToString, E>(results: impl Iterator<Item = Result<T, E>>) -> Option<Vec<String>> {
    let mut rows = results
        .map(|result| result.map(|row| row.to_string()))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    rows.sort_unstable();
    Some(rows)
}

#[test]
fn line_formats_match_oxttl() {
    let accepted = [
        r#"<urn:s> <urn:p> "text"@en--ltr ."#,
        r#"<urn:s> <urn:p> <<( _:b <urn:p> <<( <urn:a> <urn:p> "نص"@ar--rtl )>> )>> ."#,
    ];
    for line in accepted {
        let data = line.as_bytes();
        let ours = sorted(read_ntriples(data));
        assert!(ours.is_some(), "{line}");
        assert_eq!(
            ours,
            sorted(oxttl::NTriplesParser::new().for_slice(data)),
            "{line}"
        );
    }

    for line in [
        r#"<urn:s> <urn:p> <<( <urn:s> <urn:p> "نص"@ar--rtl )>> <urn:g> ."#,
        r#"<urn:s> <urn:p> "text"@en--ltr <urn:g> ."#,
    ] {
        let data = line.as_bytes();
        let ours = sorted(read_nquads(data));
        assert!(ours.is_some(), "{line}");
        assert_eq!(
            ours,
            sorted(oxttl::NQuadsParser::new().for_slice(data)),
            "{line}"
        );
    }

    for line in [
        r#"<urn:s> <urn:p> <<( <urn:a> <urn:p> <urn:b> >> ."#,
        r#"<urn:s> <urn:p> "text"@en--xx ."#,
        r#"<<( <urn:s> <urn:p> <urn:o> )>> <urn:p> <urn:o> ."#,
    ] {
        let data = line.as_bytes();
        assert_eq!(sorted(read_ntriples(data)), None, "{line}");
        assert_eq!(
            sorted(oxttl::NTriplesParser::new().for_slice(data)),
            None,
            "{line}"
        );
    }
}

#[test]
fn terse_formats_match_oxttl_on_deterministic_terms() {
    for doc in [
        r#"VERSION "1.2" PREFIX : <urn:ex:> :s :p <<( :s a <<( :s :p "text"@en--ltr )>> )>> ."#,
        r#"@version "1.2" . @prefix : <urn:ex:> . << :s :p :o ~ :r >> :source :book ."#,
        r#"PREFIX : <urn:ex:> :s :p << :a :b :c ~ :r >> ."#,
        r#"PREFIX : <urn:ex:> << << :s :p :o ~ :inner >> :p :o ~ :outer >> ."#,
        r#"PREFIX : <urn:ex:> :s :p :o ~ :r {| :source :book |}, "نص"@ar--rtl ; :p :b ."#,
    ] {
        let data = doc.as_bytes();
        let ours = sorted(read_turtle(data));
        assert!(ours.is_some(), "{doc}");
        assert_eq!(
            ours,
            sorted(oxttl::TurtleParser::new().for_slice(data)),
            "{doc}"
        );
    }
    let data = br#"PREFIX : <urn:ex:>
        :g { :s :p :o ~ :r {| :source :book |} .
             << :s :p :a ~ :r2 >> :source :book }
        { :a :p <<( :s :p "text"@en--ltr )>> }
    "#;
    let ours = sorted(read_trig(&data[..]));
    assert!(ours.is_some());
    assert_eq!(ours, sorted(oxttl::TriGParser::new().for_slice(data)));
}
