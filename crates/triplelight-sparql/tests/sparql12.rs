//! SPARQL 1.2 syntax (W3C SPARQL 1.2 Query Language, Working Draft 2026-11-19) with the `sparql-12` feature: parse
//! shapes and the expansion of reified triples and annotations (§4.3). Without the feature, the SPARQL 1.2 forms are
//! rejected as in SPARQL 1.1.

const PRE: &str = "PREFIX : <http://example/> PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> \
                   PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> ";

#[cfg(feature = "sparql-12")]
mod sparql12 {
    use super::PRE;
    use triplelight_core::rdf::{BaseDirection, Literal, NamedNode, NamedOrBlankNode, Term, Triple, Variable};
    use triplelight_sparql::*;

    // ---------- helpers ----------

    fn query(q: &str) -> Query { parse_query(&format!("{PRE}{q}"), None).unwrap_or_else(|e| panic!("{q}: {e}")) }
    fn update(u: &str) -> Update { parse_update(&format!("{PRE}{u}"), None).unwrap_or_else(|e| panic!("{u}: {e}")) }
    fn rejects(q: &str) { assert!(parse_query(&format!("{PRE}{q}"), None).is_err(), "should be rejected: {q}") }
    fn rejects_update(u: &str) { assert!(parse_update(&format!("{PRE}{u}"), None).is_err(), "should be rejected: {u}") }

    /// The pattern under `SELECT *`'s projection, and the projected variables.
    fn select(q: &str) -> (GraphPattern, Vec<Variable>) {
        match query(q) {
            Query::Select { pattern: GraphPattern::Project { inner, variables }, .. } => (*inner, variables),
            q => panic!("unexpected algebra {q:?}"),
        }
    }
    fn bgp(q: &str) -> Vec<TriplePattern> {
        match select(q).0 { GraphPattern::Bgp { patterns } => patterns, p => panic!("not a BGP: {p:?}") }
    }

    fn iri(l: &str) -> NamedNode { NamedNode::new_unchecked(format!("http://example/{l}")) }
    fn ex(l: &str) -> TermPattern { TermPattern::NamedNode(iri(l)) }
    fn exp(l: &str) -> NamedNodePattern { NamedNodePattern::NamedNode(iri(l)) }
    fn v(n: &str) -> TermPattern { TermPattern::Variable(Variable::new_unchecked(n)) }
    fn vp(n: &str) -> NamedNodePattern { NamedNodePattern::Variable(Variable::new_unchecked(n)) }
    fn lit(s: &str) -> TermPattern { TermPattern::Literal(Literal::new_simple_literal(s)) }
    fn rdf(l: &str) -> NamedNode { NamedNode::new_unchecked(format!("http://www.w3.org/1999/02/22-rdf-syntax-ns#{l}")) }
    fn reifies() -> NamedNodePattern { NamedNodePattern::NamedNode(rdf("reifies")) }
    fn tp(s: TermPattern, p: NamedNodePattern, o: TermPattern) -> TriplePattern { TriplePattern { subject: s, predicate: p, object: o } }
    fn tt(s: TermPattern, p: NamedNodePattern, o: TermPattern) -> TermPattern { TermPattern::Triple(Box::new(tp(s, p, o))) }
    fn is_bnode(t: &TermPattern) -> bool { matches!(t, TermPattern::BlankNode(_)) }

    // ---------- triple terms ----------

    #[test]
    fn triple_term_in_object_position() {
        assert_eq!(bgp(r#"SELECT * { _:anno rdf:reifies <<( ?person :jobTitle "Designer" )>> }"#)[0].object,
            tt(v("person"), exp("jobTitle"), lit("Designer")));
    }

    #[test]
    fn triple_terms_nest_and_may_be_subjects() {
        // §18.1.3: subject and object of a triple pattern may be triple patterns, recursively (a subject one never
        // matches RDF data, but it is valid syntax: VarOrTerm includes TripleTerm).
        let ps = bgp("SELECT * { <<( :a :b :c )>> :p <<( <<( ?s a ?o )>> ?p [] )>> }");
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].subject, tt(ex("a"), exp("b"), ex("c")));
        let TermPattern::Triple(o) = &ps[0].object else { panic!("{:?}", ps[0].object) };
        assert_eq!(o.subject, tt(v("s"), NamedNodePattern::NamedNode(rdf("type")), v("o")));
        assert_eq!(o.predicate, vp("p"));
        assert!(is_bnode(&o.object));
    }

    #[test]
    fn triple_term_parts_are_restricted() {
        rejects("SELECT * { ?r rdf:reifies <<( ?s :p ( :x ) )>> }"); // no collections
        rejects("SELECT * { ?r rdf:reifies <<( ?s :p [ :q :r ] )>> }"); // no blank-node property lists
        rejects("SELECT * { ?r rdf:reifies <<( ?s :p/:q ?o )>> }"); // Verb, not a path
        rejects("SELECT * { ?r rdf:reifies <<( ?s :p << :a :b :c >> )>> }"); // no reified triples inside
        rejects("SELECT * { ?r rdf:reifies <<( ?s :p ?o >> }");
        // Literals are allowed in either position of a triple-term pattern.
        assert_eq!(bgp(r#"SELECT * { ?r rdf:reifies <<( "x" :p 1 )>> }"#).len(), 1);
    }

    #[test]
    fn nested_variables_are_in_scope() {
        // §18.3.1: variable occurrence within BGPs includes variables of nested triple patterns.
        let (_, vars) = select("SELECT * { ?r rdf:reifies <<( ?s ?p <<( ?x :q ?y )>> )>> }");
        let names: Vec<&str> = vars.iter().map(Variable::as_str).collect();
        assert_eq!(names, ["r", "s", "p", "x", "y"]);
    }

    #[test]
    fn blank_node_labels_in_triple_terms_keep_their_scope() {
        rejects("SELECT * { { ?r rdf:reifies <<( _:b :p :o )>> } { _:b :q ?z } }");
        assert_eq!(bgp("SELECT * { ?r rdf:reifies <<( _:b :p :o )>> . _:b :q ?z }").len(), 2);
    }

    // ---------- reified triples (§4.3.1) ----------

    #[test]
    fn reified_triple_subject_expands_with_fresh_reifier() {
        let ps = bgp(r#"SELECT * { ?person :familyName "Smith" . << ?person :jobTitle "Designer" >> :accordingTo ?authority }"#);
        assert_eq!(ps.len(), 3);
        assert_eq!(ps[0], tp(v("person"), exp("familyName"), lit("Smith")));
        let r = &ps[1].subject;
        assert!(is_bnode(r));
        assert_eq!(ps[1], tp(r.clone(), reifies(), tt(v("person"), exp("jobTitle"), lit("Designer"))));
        assert_eq!(ps[2], tp(r.clone(), exp("accordingTo"), v("authority")));
    }

    #[test]
    fn reified_triple_with_explicit_reifier() {
        let ps = bgp(r#"SELECT * { << ?person :jobTitle "Designer" ~ ?id >> :accordingTo ?authority }"#);
        assert_eq!(ps, [
            tp(v("id"), reifies(), tt(v("person"), exp("jobTitle"), lit("Designer"))),
            tp(v("id"), exp("accordingTo"), v("authority")),
        ]);
        let ps = bgp("SELECT * { << :a :b :c ~ :r >> :p ?o }");
        assert_eq!(ps[0], tp(ex("r"), reifies(), tt(ex("a"), exp("b"), ex("c"))));
        // `~` alone, `~ []` and `~ _:x` give a blank-node reifier.
        for q in ["SELECT * { << :a :b :c ~ >> :p ?o }", "SELECT * { << :a :b :c ~ [] >> :p ?o }", "SELECT * { << :a :b :c ~ _:x >> :p ?o }"] {
            let ps = bgp(q);
            assert!(is_bnode(&ps[0].subject), "{q}");
            assert_eq!(ps[0].subject, ps[1].subject, "{q}");
        }
    }

    #[test]
    fn reified_triple_without_property_list() {
        // ReifiedTripleBlock ::= ReifiedTriple PropertyList, and PropertyList may be empty.
        assert_eq!(bgp("SELECT * { << :a :b :c ~ :r >> . ?s ?p ?o }"), [
            tp(ex("r"), reifies(), tt(ex("a"), exp("b"), ex("c"))),
            tp(v("s"), vp("p"), v("o")),
        ]);
    }

    #[test]
    fn reified_triple_in_object_position_and_collections() {
        let ps = bgp("SELECT * { ?x :says << :a :b :c >> }");
        assert_eq!(ps.len(), 2);
        assert!(is_bnode(&ps[0].subject));
        assert_eq!(ps[0], tp(ps[0].subject.clone(), reifies(), tt(ex("a"), exp("b"), ex("c"))));
        assert_eq!(ps[1], tp(v("x"), exp("says"), ps[0].subject.clone()));
        // GraphNode includes ReifiedTriple, so collections and [ … ] may contain one.
        let ps = bgp("SELECT * { ?x :list ( << :a :b :c ~ :r >> ) . [ :q << :d :e :f ~ :t >> ] :z 1 }");
        assert!(ps.contains(&tp(ex("r"), reifies(), tt(ex("a"), exp("b"), ex("c")))));
        assert!(ps.contains(&tp(ps[1].subject.clone(), NamedNodePattern::NamedNode(rdf("first")), ex("r"))));
        assert!(ps.contains(&tp(ex("t"), reifies(), tt(ex("d"), exp("e"), ex("f")))));
    }

    #[test]
    fn nested_reified_triples_expand_recursively() {
        let ps = bgp("SELECT * { << ?s1 :p1 << :s2 :p2 :o2 >> ~ :r1 >> :q ?z }");
        assert_eq!(ps.len(), 3);
        let inner = ps[0].subject.clone();
        assert!(is_bnode(&inner));
        assert_eq!(ps[0], tp(inner.clone(), reifies(), tt(ex("s2"), exp("p2"), ex("o2"))));
        assert_eq!(ps[1], tp(ex("r1"), reifies(), tt(v("s1"), exp("p1"), inner)));
        assert_eq!(ps[2], tp(ex("r1"), exp("q"), v("z")));
        // A triple term may be a part of a reified triple.
        let ps = bgp("SELECT * { << :a :b <<( :c :d :e )>> ~ :r >> }");
        assert_eq!(ps, [tp(ex("r"), reifies(), tt(ex("a"), exp("b"), tt(ex("c"), exp("d"), ex("e"))))]);
    }

    #[test]
    fn reified_triple_parts_are_restricted() {
        rejects("SELECT * { << :a :b () >> :p ?o }"); // NIL is not a ReifiedTripleObject
        rejects("SELECT * { << :a :b ( :c ) >> :p ?o }");
        rejects("SELECT * { << :a :b [ :q :r ] >> :p ?o }");
        rejects("SELECT * { << :a :b/:c :d >> :p ?o }");
        rejects("SELECT * { << :a :b :c ~ :r ~ :t >> :p ?o }"); // one optional Reifier only
        rejects("SELECT * { << :a :b :c {| :p :o |} >> }");
        assert_eq!(bgp("SELECT * { << [] :b ?c >> :p ?o }").len(), 2);
    }

    // ---------- annotations (§4.3.2) ----------

    #[test]
    fn annotation_block_without_reifier() {
        let ps = bgp(r#"SELECT * { ?person :name "Alice" {| :statedBy ?authority ; :recorded "2021-07-07"^^xsd:date |} . }"#);
        let date = TermPattern::Literal(Literal::new_typed_literal("2021-07-07", NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#date")));
        assert_eq!(ps.len(), 4);
        assert_eq!(ps[0], tp(v("person"), exp("name"), lit("Alice")));
        let r = ps[1].subject.clone();
        assert!(is_bnode(&r));
        assert_eq!(ps[1], tp(r.clone(), reifies(), tt(v("person"), exp("name"), lit("Alice"))));
        assert_eq!(ps[2], tp(r.clone(), exp("statedBy"), v("authority")));
        assert_eq!(ps[3], tp(r, exp("recorded"), date));
    }

    #[test]
    fn annotation_block_with_reifier() {
        let ps = bgp(r#"SELECT * { ?person :name "Alice" ~ :t {| :statedBy ?authority |} }"#);
        assert_eq!(ps, [
            tp(v("person"), exp("name"), lit("Alice")),
            tp(ex("t"), reifies(), tt(v("person"), exp("name"), lit("Alice"))),
            tp(ex("t"), exp("statedBy"), v("authority")),
        ]);
    }

    #[test]
    fn separate_annotation_blocks_get_separate_reifiers() {
        let ps = bgp(r#"SELECT * { ?person :name "Alice" {| :statedBy ?a1 |} {| :statedBy ?a2 |} . }"#);
        assert_eq!(ps.len(), 5);
        let (b0, b1) = (ps[1].subject.clone(), ps[3].subject.clone());
        assert!(is_bnode(&b0) && is_bnode(&b1) && b0 != b1);
        let t = tt(v("person"), exp("name"), lit("Alice"));
        assert_eq!(ps[1..], [
            tp(b0.clone(), reifies(), t.clone()), tp(b0, exp("statedBy"), v("a1")),
            tp(b1.clone(), reifies(), t), tp(b1, exp("statedBy"), v("a2")),
        ]);
        // After `~ :r {| … |}` a further block is not `:r`'s.
        let ps = bgp("SELECT * { ?s :p ?o ~ :r {| :q 1 |} {| :q 2 |} }");
        assert_eq!(ps[1].subject, ex("r"));
        assert!(is_bnode(&ps[3].subject));
    }

    #[test]
    fn several_reifiers_without_blocks() {
        let ps = bgp(r#"SELECT * { ?person :name "Alice" ~ :stmt1 ~ :stmt2 . }"#);
        let t = tt(v("person"), exp("name"), lit("Alice"));
        assert_eq!(ps, [
            tp(v("person"), exp("name"), lit("Alice")),
            tp(ex("stmt1"), reifies(), t.clone()),
            tp(ex("stmt2"), reifies(), t),
        ]);
    }

    #[test]
    fn several_reifiers_with_blocks() {
        let ps = bgp(r#"SELECT * { ?person :name "Alice" ~ :stmt1 {| :statedBy ?a1 |} ~ :stmt2 {| :statedBy ?a2 |} . }"#);
        let t = tt(v("person"), exp("name"), lit("Alice"));
        assert_eq!(ps, [
            tp(v("person"), exp("name"), lit("Alice")),
            tp(ex("stmt1"), reifies(), t.clone()), tp(ex("stmt1"), exp("statedBy"), v("a1")),
            tp(ex("stmt2"), reifies(), t), tp(ex("stmt2"), exp("statedBy"), v("a2")),
        ]);
    }

    #[test]
    fn annotations_in_object_lists_and_with_variable_or_a_predicates() {
        let ps = bgp("SELECT * { ?s ?p ?o ~ ?r , ?o2 ; a :C ~ :t }");
        assert_eq!(ps, [
            tp(v("s"), vp("p"), v("o")),
            tp(v("r"), reifies(), tt(v("s"), vp("p"), v("o"))),
            tp(v("s"), vp("p"), v("o2")),
            tp(v("s"), NamedNodePattern::NamedNode(rdf("type")), ex("C")),
            tp(ex("t"), reifies(), tt(v("s"), NamedNodePattern::NamedNode(rdf("type")), ex("C"))),
        ]);
        // Annotations nest: an annotation block's own objects may carry annotations.
        let ps = bgp("SELECT * { :s :p :o ~ :r {| :q :x ~ :r2 |} }");
        assert_eq!(ps[3], tp(ex("r2"), reifies(), tt(ex("r"), exp("q"), ex("x"))));
    }

    #[test]
    fn annotations_need_a_simple_predicate() {
        // §19.7 notes: only after an IRI, `a` or a variable, not other path expressions.
        for q in ["SELECT * { ?s :p/:q ?o ~ :r }", "SELECT * { ?s :p|:q ?o {| :z 1 |} }", "SELECT * { ?s :p* ?o ~ :r }",
            "SELECT * { ?s ^:p ?o ~ :r }", "SELECT * { ?s !:p ?o {| :z 1 |} }"] {
            rejects(q);
        }
        // An annotation block itself is a PropertyListPathNotEmpty in a WHERE clause: paths are allowed inside it.
        match select("SELECT * { ?s :p ?o {| :q+ ?z |} }").0 {
            GraphPattern::Join { left, right } => {
                let GraphPattern::Bgp { patterns } = *left else { panic!() };
                assert_eq!(patterns.len(), 2);
                let GraphPattern::Path { subject, path, object } = *right else { panic!() };
                assert_eq!(subject, patterns[1].subject);
                assert_eq!(path, PropertyPathExpression::OneOrMore(Box::new(PropertyPathExpression::NamedNode(iri("q")))));
                assert_eq!(object, v("z"));
            }
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn unterminated_annotations_are_rejected() {
        rejects("SELECT * { ?s :p ?o {| :q ?z } }");
        rejects("SELECT * { ?s :p ?o {| |} }");
        rejects("SELECT * { ?s :p ?o ~ :r :q }");
    }

    // ---------- literals with a base direction ----------

    #[test]
    fn directional_language_tagged_literals() {
        let ps = bgp(r#"SELECT * { ?s :p "abc"@en--ltr , "قطة"@AR--rtl , "x"@en-GB }"#);
        let lit = |t: &TermPattern| match t { TermPattern::Literal(l) => l.clone(), t => panic!("{t:?}") };
        assert_eq!(lit(&ps[0].object), Literal::new_directional_language_tagged_literal_unchecked("abc", "en", BaseDirection::Ltr));
        assert_eq!(lit(&ps[1].object), Literal::new_directional_language_tagged_literal_unchecked("قطة", "ar", BaseDirection::Rtl));
        assert_eq!(lit(&ps[1].object).direction(), Some(BaseDirection::Rtl));
        assert_eq!(lit(&ps[2].object), Literal::new_language_tagged_literal_unchecked("x", "en-gb"));
        // The direction is always lower case and either ltr or rtl (§2.3.1).
        rejects(r#"SELECT * { ?s :p "abc"@en--LTR }"#);
        rejects(r#"SELECT * { ?s :p "abc"@en--up }"#);
        rejects(r#"SELECT * { ?s :p "abc"@en-- }"#);
        // In expressions too.
        let (p, _) = select(r#"SELECT * { FILTER(?x = "a"@he--rtl) }"#);
        let GraphPattern::Filter { expr: Expression::Equal(_, b), .. } = p else { panic!("{p:?}") };
        assert_eq!(*b, Expression::Literal(Literal::new_directional_language_tagged_literal_unchecked("a", "he", BaseDirection::Rtl)));
    }

    // ---------- expressions ----------

    #[test]
    fn expression_triple_term_is_triple_call() {
        let (p, _) = select(r#"SELECT * { BIND(<<( ?s :p <<( :a a "o" )>> )>> AS ?t) }"#);
        let GraphPattern::Extend { expression, variable, .. } = p else { panic!("{p:?}") };
        assert_eq!(variable.as_str(), "t");
        let inner = Expression::FunctionCall(Function::Triple, vec![
            Expression::NamedNode(iri("a")), Expression::NamedNode(rdf("type")), Expression::Literal(Literal::new_simple_literal("o")),
        ]);
        assert_eq!(expression, Expression::FunctionCall(Function::Triple, vec![
            Expression::Variable(Variable::new_unchecked("s")), Expression::NamedNode(iri("p")), inner,
        ]));
        // ExprTripleTermSubject is `iri | Var` only; no blank nodes anywhere.
        rejects(r#"SELECT * { BIND(<<( "s" :p :o )>> AS ?t) }"#);
        rejects("SELECT * { BIND(<<( <<( :a :b :c )>> :p :o )>> AS ?t) }");
        rejects("SELECT * { BIND(<<( :s :p _:b )>> AS ?t) }");
        rejects("SELECT * { BIND(<<( :s :p/:q :o )>> AS ?t) }");
    }

    #[test]
    fn sparql12_builtin_functions() {
        let cases = [
            ("TRIPLE(?s, ?p, ?o)", Function::Triple, 3), ("SUBJECT(?t)", Function::Subject, 1),
            ("PREDICATE(?t)", Function::Predicate, 1), ("OBJECT(?t)", Function::Object, 1),
            ("isTRIPLE(?t)", Function::IsTriple, 1), ("LANGDIR(?l)", Function::LangDir, 1),
            ("hasLANG(?l)", Function::HasLang, 1), ("hasLANGDIR(?l)", Function::HasLangDir, 1),
            ("STRLANGDIR(?l, \"en\", \"ltr\")", Function::StrLangDir, 3), ("istriple(?t)", Function::IsTriple, 1),
        ];
        for (call, f, n) in cases {
            let (p, _) = select(&format!("SELECT * {{ BIND({call} AS ?x) }}"));
            let GraphPattern::Extend { expression: Expression::FunctionCall(g, args), .. } = p else { panic!("{call}: {p:?}") };
            assert_eq!((g, args.len()), (f, n), "{call}");
        }
        for bad in ["TRIPLE(?s, ?p)", "SUBJECT()", "OBJECT(?a, ?b)", "LANGDIR(?a, ?b)", "STRLANGDIR(?a, ?b)", "hasLANG()"] {
            rejects(&format!("SELECT * {{ BIND({bad} AS ?x) }}"));
        }
        assert_eq!(Function::IsTriple.to_string(), "isTRIPLE");
        assert_eq!(Function::StrLangDir.to_string(), "STRLANGDIR");
        assert_eq!(Function::HasLangDir.to_string(), "hasLANGDIR");
        // FILTER constraints accept them as BuiltInCall.
        assert!(matches!(select("SELECT * { ?s ?p ?o FILTER isTRIPLE(?o) }").0, GraphPattern::Filter { .. }));
    }

    // ---------- VALUES ----------

    #[test]
    fn values_triple_terms() {
        let (p, _) = select(r#"SELECT * { VALUES ?t { <<( :a :b "c" )>> <<( :a a <<( :x :y :z )>> )>> UNDEF } }"#);
        let GraphPattern::Values { bindings, .. } = p else { panic!("{p:?}") };
        let g = |s: &str, p: NamedNode, o: GroundTerm| GroundTerm::Triple(Box::new(GroundTriple { subject: iri(s), predicate: p, object: o }));
        assert_eq!(bindings, [
            vec![Some(g("a", iri("b"), GroundTerm::Literal(Literal::new_simple_literal("c"))))],
            vec![Some(g("a", rdf("type"), g("x", iri("y"), GroundTerm::NamedNode(iri("z")))))],
            vec![None],
        ]);
        // TripleTermData: IRI subject, no variables or blank nodes.
        rejects("SELECT * { VALUES ?t { <<( ?s :b :c )>> } }");
        rejects("SELECT * { VALUES ?t { <<( :a :b _:c )>> } }");
        rejects(r#"SELECT * { VALUES ?t { <<( "a" :b :c )>> } }"#);
        rejects("SELECT * { VALUES ?t { <<( <<( :a :b :c )>> :b :c )>> } }");
        rejects("SELECT * { VALUES ?t { << :a :b :c >> } }");
    }

    // ---------- VERSION ----------

    #[test]
    fn version_declaration() {
        for q in [r#"VERSION "1.2" SELECT * {}"#, "VERSION '1.2-basic' SELECT * {}", r#"BASE <http://b/> VERSION "1.1" PREFIX p: <x> VERSION "x" SELECT * {}"#] {
            parse_query(q, None).unwrap_or_else(|e| panic!("{q}: {e}"));
        }
        for q in [r#"VERSION """1.2""" SELECT * {}"#, "VERSION '''1.2''' SELECT * {}", "VERSION 1.2 SELECT * {}", "VERSION SELECT * {}", "VERSION <x> SELECT * {}"] {
            assert!(parse_query(q, None).is_err(), "{q}");
        }
        parse_update(r#"VERSION "1.2" INSERT DATA { <http://a> <http://b> <http://c> } ; VERSION "1.2" CLEAR ALL"#, None).unwrap();
    }

    // ---------- CONSTRUCT templates ----------

    #[test]
    fn construct_template_triple_terms_and_annotations() {
        let Query::Construct { template, .. } = query("CONSTRUCT { ?s :p <<( ?s :q ?o )>> . << ?s :q ?o >> :src :x . ?s :q ?o {| :src :y |} } WHERE { ?s :q ?o }") else { panic!() };
        let t = tt(v("s"), exp("q"), v("o"));
        assert_eq!(template.len(), 6);
        assert_eq!(template[0], tp(v("s"), exp("p"), t.clone()));
        let b1 = template[1].subject.clone();
        assert!(is_bnode(&b1));
        assert_eq!(template[1..3], [tp(b1.clone(), reifies(), t.clone()), tp(b1.clone(), exp("src"), ex("x"))]);
        assert_eq!(template[3], tp(v("s"), exp("q"), v("o")));
        let b2 = template[4].subject.clone();
        assert!(is_bnode(&b2) && b2 != b1);
        assert_eq!(template[4..], [tp(b2.clone(), reifies(), t), tp(b2, exp("src"), ex("y"))]);
        // CONSTRUCT WHERE: the template is the (expanded) pattern.
        let Query::Construct { template, .. } = query("CONSTRUCT WHERE { ?s :q ?o ~ ?r }") else { panic!() };
        assert_eq!(template[1], tp(v("r"), reifies(), tt(v("s"), exp("q"), v("o"))));
    }

    // ---------- updates ----------

    #[test]
    fn insert_data_triple_terms() {
        let u = update("INSERT DATA { :s :p <<( :a :b <<( _:x :c 1 )>> )>> . << :a :b :c ~ :r >> :q :z }");
        let GraphUpdateOperation::InsertData { data } = &u.operations[0] else { panic!() };
        assert_eq!(data.len(), 3);
        let Term::Triple(t) = &data[0].object else { panic!("{:?}", data[0].object) };
        assert_eq!(t.subject, NamedOrBlankNode::from(iri("a")));
        let Term::Triple(inner) = &t.object else { panic!() };
        assert!(inner.subject.is_blank_node());
        assert_eq!(data[1].object, Term::Triple(Box::new(Triple::new(iri("a"), iri("b"), iri("c")))));
        assert_eq!(data[1].predicate, rdf("reifies"));
        rejects_update("INSERT DATA { :s :p <<( ?a :b :c )>> }");
        rejects_update(r#"INSERT DATA { :s :p <<( "a" :b :c )>> }"#);
        rejects_update("INSERT DATA { :s :p <<( <<( :a :b :c )>> :b :c )>> }");
        rejects_update("INSERT DATA { <<( :a :b :c )>> :p :o }"); // a triple term cannot be a subject in RDF
        // Labels nested in triple terms are shared across INSERT DATA operations like top-level ones.
        rejects_update("INSERT DATA { :s :p _:b } ; INSERT DATA { :s :p <<( _:b :c :d )>> }");
    }

    #[test]
    fn delete_data_triple_terms() {
        let u = update("DELETE DATA { :s :p <<( :a :b <<( :c :d \"e\" )>> )>> }");
        let GraphUpdateOperation::DeleteData { data } = &u.operations[0] else { panic!() };
        let inner = GroundTerm::Triple(Box::new(GroundTriple { subject: iri("c"), predicate: iri("d"), object: GroundTerm::Literal(Literal::new_simple_literal("e")) }));
        assert_eq!(data[0].object, GroundTerm::Triple(Box::new(GroundTriple { subject: iri("a"), predicate: iri("b"), object: inner })));
        rejects_update("DELETE DATA { :s :p <<( _:a :b :c )>> }");
        rejects_update("DELETE DATA { :s :p :o {| :q :r |} }"); // the fresh reifier is a blank node
        assert!(parse_update(&format!("{PRE}DELETE DATA {{ :s :p :o ~ :r {{| :q :z |}} }}"), None).is_ok());
    }

    #[test]
    fn delete_templates_and_delete_where() {
        let u = update("DELETE { ?s :p <<( ?s :q ?o )>> } WHERE { ?s :p ?o }");
        let GraphUpdateOperation::DeleteInsert { delete, .. } = &u.operations[0] else { panic!() };
        let GroundTermPattern::Triple(t) = &delete[0].object else { panic!("{:?}", delete[0].object) };
        assert_eq!(t.subject, GroundTermPattern::Variable(Variable::new_unchecked("s")));
        assert_eq!(t.object, GroundTermPattern::Variable(Variable::new_unchecked("o")));
        rejects_update("DELETE { ?s :p <<( [] :q ?o )>> } WHERE { ?s :p ?o }");
        rejects_update("DELETE WHERE { ?s :p ?o {| :q ?z |} }"); // blank-node reifier
        rejects_update("DELETE WHERE { ?s :p ?o ~ }");
        let u = update("DELETE WHERE { ?s :p ?o ~ ?r {| :q ?z |} }");
        let GraphUpdateOperation::DeleteInsert { delete, pattern, .. } = &u.operations[0] else { panic!() };
        assert_eq!(delete.len(), 3);
        let GraphPattern::Bgp { patterns } = &**pattern else { panic!("{pattern:?}") };
        assert_eq!(patterns[1], tp(v("r"), reifies(), tt(v("s"), exp("p"), v("o"))));
        // INSERT templates may use fresh reifiers.
        let u = update("INSERT { ?s :p ?o {| :q 1 |} } WHERE { ?s :p ?o }");
        let GraphUpdateOperation::DeleteInsert { insert, .. } = &u.operations[0] else { panic!() };
        assert_eq!(insert.len(), 3);
        assert!(is_bnode(&insert[1].subject));
    }
}

/// Without the feature, the SPARQL 1.2 syntax stays a syntax error, as in SPARQL 1.1.
#[cfg(not(feature = "sparql-12"))]
mod sparql11 {
    use super::PRE;
    use triplelight_sparql::*;

    fn ok(q: &str) { parse_query(&format!("{PRE}{q}"), None).unwrap_or_else(|e| panic!("{q}: {e}")); }
    fn rejects(q: &str) { assert!(parse_query(&format!("{PRE}{q}"), None).is_err(), "should be rejected: {q}") }

    #[test]
    fn sparql12_syntax_is_rejected() {
        for q in [
            "SELECT * { ?r rdf:reifies <<( :a :b :c )>> }",
            "SELECT * { << :a :b :c >> :p ?o }",
            "SELECT * { ?x :says << :a :b :c >> }",
            "SELECT * { << :a :b :c ~ :r >> }",
            "SELECT * { ?s :p ?o ~ :r }",
            "SELECT * { ?s :p ?o {| :q ?z |} }",
            r#"SELECT * { ?s :p "abc"@en--ltr }"#,
            r#"VERSION "1.2" SELECT * {}"#,
            "SELECT * { BIND(<<( ?s :p :o )>> AS ?t) }",
            "SELECT * { VALUES ?t { <<( :a :b :c )>> } }",
            "SELECT (isTRIPLE(?o) AS ?x) { ?s ?p ?o }",
            "SELECT (TRIPLE(?s, ?p, ?o) AS ?x) { ?s ?p ?o }",
            "SELECT (LANGDIR(?o) AS ?x) { ?s ?p ?o }",
            "SELECT * { ?s ?p ?o FILTER hasLANG(?o) }",
        ] {
            rejects(q);
        }
        assert!(parse_update(&format!("{PRE}INSERT DATA {{ :s :p <<( :a :b :c )>> }}"), None).is_err());
    }

    #[test]
    fn sparql11_neighbours_still_parse() {
        ok(r#"SELECT * { ?s :p "abc"@en-us }"#);
        ok("SELECT * { ?s ?p ?o FILTER(?o < <http://example/x>) }");
        ok("SELECT * { ?s ?p ?o FILTER(?o<<http://example/x>) }"); // `<` then an IRI in SPARQL 1.1
        ok("SELECT * { ?s :p|:q ?o }");
    }
}

/// W3C SPARQL 1.2 conformance fixes: `!!?x` parses (UnaryExpression recursion), and a VALUES header with a repeated
/// variable is a syntax error.
#[cfg(feature = "sparql-12")]
mod w3c_fixes {
    #[test]
    fn double_negation_parses() {
        assert!(triplelight_sparql::parse_query("SELECT ?e { VALUES ?v { true } BIND(!!?v AS ?e) }", None).is_ok());
    }
    #[test]
    fn duplicated_values_variable_is_rejected() {
        assert!(triplelight_sparql::parse_query("SELECT * WHERE { VALUES (?a ?a) { (1 1) } }", None).is_err());
        assert!(triplelight_sparql::parse_query("SELECT * WHERE { VALUES (?a ?b) { (1 1) } }", None).is_ok());
    }
}
