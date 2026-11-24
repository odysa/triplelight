//! An immutable, indexed in-memory RDF graph: the data graph and the shapes graph are both one of these.
use super::error::ValidationError;
use super::vocab::{rdf, rdfs};
use crate::rdf::{NamedNode, Term, Triple};
use std::collections::{HashMap, HashSet, VecDeque};

pub struct Graph {
    /// Subject -> predicate -> objects, in the order the triples were given (duplicates dropped).
    spo: HashMap<Term, HashMap<NamedNode, Vec<Term>>>,
    /// Subject -> every (predicate, object) pair.
    outgoing: HashMap<Term, Vec<(NamedNode, Term)>>,
    /// Predicate -> object -> subjects.
    pos: HashMap<NamedNode, HashMap<Term, Vec<Term>>>,
    /// Predicate -> its subjects, then its objects, in first-seen order.
    predicate_subjects: HashMap<NamedNode, Vec<Term>>,
    predicate_objects: HashMap<NamedNode, Vec<Term>>,
    rdf_type: NamedNode,
    sub_class_of: NamedNode,
}

impl Graph {
    pub fn new(triples: &[Triple]) -> Graph {
        let mut g = Graph {
            spo: HashMap::new(), outgoing: HashMap::new(), pos: HashMap::new(),
            predicate_subjects: HashMap::new(), predicate_objects: HashMap::new(),
            rdf_type: rdf("type"), sub_class_of: rdfs("subClassOf"),
        };
        let mut seen: HashSet<&Triple> = HashSet::new();
        let (mut sub_seen, mut obj_seen): (HashSet<(&NamedNode, Term)>, HashSet<(&NamedNode, &Term)>) = (HashSet::new(), HashSet::new());
        for t in triples {
            if !seen.insert(t) { continue }
            let s = Term::from(t.subject.clone());
            g.spo.entry(s.clone()).or_default().entry(t.predicate.clone()).or_default().push(t.object.clone());
            g.outgoing.entry(s.clone()).or_default().push((t.predicate.clone(), t.object.clone()));
            g.pos.entry(t.predicate.clone()).or_default().entry(t.object.clone()).or_default().push(s.clone());
            if sub_seen.insert((&t.predicate, s.clone())) { g.predicate_subjects.entry(t.predicate.clone()).or_default().push(s) }
            if obj_seen.insert((&t.predicate, &t.object)) { g.predicate_objects.entry(t.predicate.clone()).or_default().push(t.object.clone()) }
        }
        g
    }

    /// The objects of `(s, p, ?)`, in input order; empty if there are none.
    pub fn objects(&self, s: &Term, p: &NamedNode) -> &[Term] {
        self.spo.get(s).and_then(|m| m.get(p)).map_or(&[], Vec::as_slice)
    }

    /// The single object of `(s, p, ?)`, `None` if there is none, an error if there are several.
    pub fn object(&self, s: &Term, p: &NamedNode) -> Result<Option<&Term>, ValidationError> {
        match self.objects(s, p) {
            [] => Ok(None),
            [one] => Ok(Some(one)),
            _ => Err(ValidationError::MalformedShape(format!("{s} has more than one {p}"))),
        }
    }

    /// The subjects of `(?, p, o)`.
    pub fn subjects(&self, p: &NamedNode, o: &Term) -> Vec<Term> {
        self.pos.get(p).and_then(|m| m.get(o)).cloned().unwrap_or_default()
    }

    /// Every (predicate, object) pair of `s`.
    pub fn outgoing(&self, s: &Term) -> &[(NamedNode, Term)] {
        self.outgoing.get(s).map_or(&[], Vec::as_slice)
    }

    /// Every subject that has predicate `p`, once, in first-seen order.
    pub fn subjects_of(&self, p: &NamedNode) -> &[Term] {
        self.predicate_subjects.get(p).map_or(&[], Vec::as_slice)
    }

    /// Every object of predicate `p`, once, in first-seen order.
    pub fn objects_of(&self, p: &NamedNode) -> &[Term] {
        self.predicate_objects.get(p).map_or(&[], Vec::as_slice)
    }

    /// `class` and every class below it through `rdfs:subClassOf` (the relation's reverse closure), cycle-safe.
    fn classes_at_or_below(&self, class: &Term) -> Vec<Term> {
        let mut out = vec![class.clone()];
        let mut seen: HashSet<Term> = HashSet::from([class.clone()]);
        let mut queue = VecDeque::from([class.clone()]);
        while let Some(c) = queue.pop_front() {
            for sub in self.subjects(&self.sub_class_of, &c) {
                if seen.insert(sub.clone()) { out.push(sub.clone()); queue.push_back(sub) }
            }
        }
        out
    }

    /// SHACL class membership: `node` has an `rdf:type` that is `class` or a (transitive) subclass of it.
    pub fn is_instance(&self, node: &Term, class: &Term) -> bool {
        let below = self.classes_at_or_below(class);
        self.objects(node, &self.rdf_type).iter().any(|t| below.contains(t))
    }

    /// Every SHACL instance of `class`, once, in first-seen order.
    pub fn instances_of(&self, class: &Term) -> Vec<Term> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for c in self.classes_at_or_below(class) {
            for n in self.subjects(&self.rdf_type, &c) { if seen.insert(n.clone()) { out.push(n) } }
        }
        out
    }

    /// The members of the `rdf:List` starting at `head`. A list must end in `rdf:nil`, have exactly one `rdf:first` and
    /// one `rdf:rest` per cell, and not loop.
    pub fn list_items(&self, head: &Term) -> Result<Vec<Term>, ValidationError> {
        let (first, rest, nil) = (rdf("first"), rdf("rest"), Term::from(rdf("nil")));
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        let mut cell = head.clone();
        while cell != nil {
            if !seen.insert(cell.clone()) { return Err(ValidationError::MalformedList(format!("{head} loops"))) }
            let item = self.object(&cell, &first).map_err(|_| ValidationError::MalformedList(format!("{cell} has several rdf:first")))?
                .ok_or_else(|| ValidationError::MalformedList(format!("{cell} has no rdf:first")))?;
            items.push(item.clone());
            cell = self.object(&cell, &rest).map_err(|_| ValidationError::MalformedList(format!("{cell} has several rdf:rest")))?
                .ok_or_else(|| ValidationError::MalformedList(format!("{cell} has no rdf:rest")))?.clone();
        }
        Ok(items)
    }
}
