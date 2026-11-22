// Derived from oxrdf 0.3.5 (MIT); see THIRD_PARTY_NOTICES.md.
use crate::blank_node::BlankNode;
use crate::literal::Literal;
use crate::named_node::NamedNode;
use crate::{BlankNodeRef, LiteralRef, NamedNodeRef};
use std::fmt;

#[derive(Eq, PartialEq, Debug, Clone, Hash)]
pub enum NamedOrBlankNode {
    NamedNode(NamedNode),
    BlankNode(BlankNode),
}

impl NamedOrBlankNode {
    #[inline]
    pub fn is_named_node(&self) -> bool {
        self.as_ref().is_named_node()
    }

    #[inline]
    pub fn is_blank_node(&self) -> bool {
        self.as_ref().is_blank_node()
    }

    #[inline]
    pub fn as_ref(&self) -> NamedOrBlankNodeRef<'_> {
        match self {
            Self::NamedNode(node) => NamedOrBlankNodeRef::NamedNode(node.as_ref()),
            Self::BlankNode(node) => NamedOrBlankNodeRef::BlankNode(node.as_ref()),
        }
    }
}

impl fmt::Display for NamedOrBlankNode {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl From<NamedNode> for NamedOrBlankNode {
    #[inline]
    fn from(node: NamedNode) -> Self {
        Self::NamedNode(node)
    }
}

impl From<NamedNodeRef<'_>> for NamedOrBlankNode {
    #[inline]
    fn from(node: NamedNodeRef<'_>) -> Self {
        node.into_owned().into()
    }
}

impl From<BlankNode> for NamedOrBlankNode {
    #[inline]
    fn from(node: BlankNode) -> Self {
        Self::BlankNode(node)
    }
}

impl From<BlankNodeRef<'_>> for NamedOrBlankNode {
    #[inline]
    fn from(node: BlankNodeRef<'_>) -> Self {
        node.into_owned().into()
    }
}

#[derive(Eq, PartialEq, Debug, Clone, Copy, Hash)]
pub enum NamedOrBlankNodeRef<'a> {
    NamedNode(NamedNodeRef<'a>),
    BlankNode(BlankNodeRef<'a>),
}

impl NamedOrBlankNodeRef<'_> {
    #[inline]
    pub fn is_named_node(&self) -> bool {
        match self {
            Self::NamedNode(_) => true,
            Self::BlankNode(_) => false,
        }
    }

    #[inline]
    pub fn is_blank_node(&self) -> bool {
        match self {
            Self::NamedNode(_) => false,
            Self::BlankNode(_) => true,
        }
    }

    #[inline]
    pub fn into_owned(self) -> NamedOrBlankNode {
        match self {
            Self::NamedNode(node) => NamedOrBlankNode::NamedNode(node.into_owned()),
            Self::BlankNode(node) => NamedOrBlankNode::BlankNode(node.into_owned()),
        }
    }
}

impl fmt::Display for NamedOrBlankNodeRef<'_> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NamedNode(node) => node.fmt(f),
            Self::BlankNode(node) => node.fmt(f),
        }
    }
}

impl<'a> From<NamedNodeRef<'a>> for NamedOrBlankNodeRef<'a> {
    #[inline]
    fn from(node: NamedNodeRef<'a>) -> Self {
        Self::NamedNode(node)
    }
}

impl<'a> From<&'a NamedNode> for NamedOrBlankNodeRef<'a> {
    #[inline]
    fn from(node: &'a NamedNode) -> Self {
        node.as_ref().into()
    }
}

impl<'a> From<BlankNodeRef<'a>> for NamedOrBlankNodeRef<'a> {
    #[inline]
    fn from(node: BlankNodeRef<'a>) -> Self {
        Self::BlankNode(node)
    }
}

impl<'a> From<&'a BlankNode> for NamedOrBlankNodeRef<'a> {
    #[inline]
    fn from(node: &'a BlankNode) -> Self {
        node.as_ref().into()
    }
}

impl<'a> From<&'a NamedOrBlankNode> for NamedOrBlankNodeRef<'a> {
    #[inline]
    fn from(node: &'a NamedOrBlankNode) -> Self {
        node.as_ref()
    }
}

impl<'a> From<NamedOrBlankNodeRef<'a>> for NamedOrBlankNode {
    #[inline]
    fn from(node: NamedOrBlankNodeRef<'a>) -> Self {
        node.into_owned()
    }
}

#[derive(Eq, PartialEq, Debug, Clone, Hash)]
pub enum Term {
    NamedNode(NamedNode),
    BlankNode(BlankNode),
    Literal(Literal),
}

impl Term {
    #[inline]
    pub fn is_named_node(&self) -> bool {
        self.as_ref().is_named_node()
    }

    #[inline]
    pub fn is_blank_node(&self) -> bool {
        self.as_ref().is_blank_node()
    }

    #[inline]
    pub fn is_literal(&self) -> bool {
        self.as_ref().is_literal()
    }


    #[inline]
    pub fn as_ref(&self) -> TermRef<'_> {
        match self {
            Self::NamedNode(node) => TermRef::NamedNode(node.as_ref()),
            Self::BlankNode(node) => TermRef::BlankNode(node.as_ref()),
            Self::Literal(literal) => TermRef::Literal(literal.as_ref()),
        }
    }
}

impl fmt::Display for Term {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl From<NamedNode> for Term {
    #[inline]
    fn from(node: NamedNode) -> Self {
        Self::NamedNode(node)
    }
}

impl From<NamedNodeRef<'_>> for Term {
    #[inline]
    fn from(node: NamedNodeRef<'_>) -> Self {
        node.into_owned().into()
    }
}

impl From<BlankNode> for Term {
    #[inline]
    fn from(node: BlankNode) -> Self {
        Self::BlankNode(node)
    }
}

impl From<BlankNodeRef<'_>> for Term {
    #[inline]
    fn from(node: BlankNodeRef<'_>) -> Self {
        node.into_owned().into()
    }
}

impl From<Literal> for Term {
    #[inline]
    fn from(literal: Literal) -> Self {
        Self::Literal(literal)
    }
}

impl From<LiteralRef<'_>> for Term {
    #[inline]
    fn from(literal: LiteralRef<'_>) -> Self {
        literal.into_owned().into()
    }
}




impl From<NamedOrBlankNode> for Term {
    #[inline]
    fn from(node: NamedOrBlankNode) -> Self {
        match node {
            NamedOrBlankNode::NamedNode(node) => node.into(),
            NamedOrBlankNode::BlankNode(node) => node.into(),
        }
    }
}

impl From<NamedOrBlankNodeRef<'_>> for Term {
    #[inline]
    fn from(node: NamedOrBlankNodeRef<'_>) -> Self {
        node.into_owned().into()
    }
}

impl TryFrom<Term> for NamedNode {
    type Error = TryFromTermError;

    #[inline]
    fn try_from(term: Term) -> Result<Self, Self::Error> {
        if let Term::NamedNode(node) = term {
            Ok(node)
        } else {
            Err(TryFromTermError {
                term,
                target: "NamedNode",
            })
        }
    }
}

impl TryFrom<Term> for BlankNode {
    type Error = TryFromTermError;

    #[inline]
    fn try_from(term: Term) -> Result<Self, Self::Error> {
        if let Term::BlankNode(node) = term {
            Ok(node)
        } else {
            Err(TryFromTermError {
                term,
                target: "BlankNode",
            })
        }
    }
}

impl TryFrom<Term> for Literal {
    type Error = TryFromTermError;

    #[inline]
    fn try_from(term: Term) -> Result<Self, Self::Error> {
        if let Term::Literal(node) = term {
            Ok(node)
        } else {
            Err(TryFromTermError {
                term,
                target: "Literal",
            })
        }
    }
}

impl TryFrom<Term> for NamedOrBlankNode {
    type Error = TryFromTermError;

    #[inline]
    fn try_from(term: Term) -> Result<Self, Self::Error> {
        match term {
            Term::NamedNode(term) => Ok(Self::NamedNode(term)),
            Term::BlankNode(term) => Ok(Self::BlankNode(term)),
            Term::Literal(_) => Err(TryFromTermError {
                term,
                target: "NamedOrBlankNode",
            }),
        }
    }
}

#[derive(Eq, PartialEq, Debug, Clone, Copy, Hash)]
pub enum TermRef<'a> {
    NamedNode(NamedNodeRef<'a>),
    BlankNode(BlankNodeRef<'a>),
    Literal(LiteralRef<'a>),
}

impl TermRef<'_> {
    #[inline]
    pub fn is_named_node(&self) -> bool {
        matches!(self, Self::NamedNode(_))
    }

    #[inline]
    pub fn is_blank_node(&self) -> bool {
        matches!(self, Self::BlankNode(_))
    }

    #[inline]
    pub fn is_literal(&self) -> bool {
        matches!(self, Self::Literal(_))
    }


    #[inline]
    pub fn into_owned(self) -> Term {
        match self {
            Self::NamedNode(node) => Term::NamedNode(node.into_owned()),
            Self::BlankNode(node) => Term::BlankNode(node.into_owned()),
            Self::Literal(literal) => Term::Literal(literal.into_owned()),
        }
    }
}

impl fmt::Display for TermRef<'_> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NamedNode(node) => node.fmt(f),
            Self::BlankNode(node) => node.fmt(f),
            Self::Literal(literal) => literal.fmt(f),
        }
    }
}

impl<'a> From<NamedNodeRef<'a>> for TermRef<'a> {
    #[inline]
    fn from(node: NamedNodeRef<'a>) -> Self {
        Self::NamedNode(node)
    }
}

impl<'a> From<&'a NamedNode> for TermRef<'a> {
    #[inline]
    fn from(node: &'a NamedNode) -> Self {
        node.as_ref().into()
    }
}

impl<'a> From<BlankNodeRef<'a>> for TermRef<'a> {
    #[inline]
    fn from(node: BlankNodeRef<'a>) -> Self {
        Self::BlankNode(node)
    }
}

impl<'a> From<&'a BlankNode> for TermRef<'a> {
    #[inline]
    fn from(node: &'a BlankNode) -> Self {
        node.as_ref().into()
    }
}

impl<'a> From<LiteralRef<'a>> for TermRef<'a> {
    #[inline]
    fn from(literal: LiteralRef<'a>) -> Self {
        Self::Literal(literal)
    }
}

impl<'a> From<&'a Literal> for TermRef<'a> {
    #[inline]
    fn from(literal: &'a Literal) -> Self {
        literal.as_ref().into()
    }
}


impl<'a> From<NamedOrBlankNodeRef<'a>> for TermRef<'a> {
    #[inline]
    fn from(node: NamedOrBlankNodeRef<'a>) -> Self {
        match node {
            NamedOrBlankNodeRef::NamedNode(node) => node.into(),
            NamedOrBlankNodeRef::BlankNode(node) => node.into(),
        }
    }
}

impl<'a> From<&'a NamedOrBlankNode> for TermRef<'a> {
    #[inline]
    fn from(node: &'a NamedOrBlankNode) -> Self {
        node.as_ref().into()
    }
}

impl<'a> From<&'a Term> for TermRef<'a> {
    #[inline]
    fn from(node: &'a Term) -> Self {
        node.as_ref()
    }
}

impl<'a> From<TermRef<'a>> for Term {
    #[inline]
    fn from(node: TermRef<'a>) -> Self {
        node.into_owned()
    }
}

#[derive(Eq, PartialEq, Debug, Clone, Hash)]
pub struct Triple {
    pub subject: NamedOrBlankNode,

    pub predicate: NamedNode,

    pub object: Term,
}

impl Triple {
    #[inline]
    pub fn new(
        subject: impl Into<NamedOrBlankNode>,
        predicate: impl Into<NamedNode>,
        object: impl Into<Term>,
    ) -> Self {
        Self {
            subject: subject.into(),
            predicate: predicate.into(),
            object: object.into(),
        }
    }

    #[inline]
    pub fn from_terms(
        subject: impl Into<Term>,
        predicate: impl Into<Term>,
        object: impl Into<Term>,
    ) -> Result<Self, TryFromTermError> {
        Ok(Self {
            subject: subject.into().try_into()?,
            predicate: predicate.into().try_into()?,
            object: object.into(),
        })
    }

    #[inline]
    pub fn in_graph(self, graph_name: impl Into<GraphName>) -> Quad {
        Quad {
            subject: self.subject,
            predicate: self.predicate,
            object: self.object,
            graph_name: graph_name.into(),
        }
    }

    #[inline]
    pub fn as_ref(&self) -> TripleRef<'_> {
        TripleRef {
            subject: self.subject.as_ref(),
            predicate: self.predicate.as_ref(),
            object: self.object.as_ref(),
        }
    }
}

impl fmt::Display for Triple {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}


#[derive(Eq, PartialEq, Debug, Clone, Copy, Hash)]
pub struct TripleRef<'a> {
    pub subject: NamedOrBlankNodeRef<'a>,

    pub predicate: NamedNodeRef<'a>,

    pub object: TermRef<'a>,
}

impl<'a> TripleRef<'a> {
    #[inline]
    pub fn new(
        subject: impl Into<NamedOrBlankNodeRef<'a>>,
        predicate: impl Into<NamedNodeRef<'a>>,
        object: impl Into<TermRef<'a>>,
    ) -> Self {
        Self {
            subject: subject.into(),
            predicate: predicate.into(),
            object: object.into(),
        }
    }

    #[inline]
    pub fn in_graph(self, graph_name: impl Into<GraphNameRef<'a>>) -> QuadRef<'a> {
        QuadRef {
            subject: self.subject,
            predicate: self.predicate,
            object: self.object,
            graph_name: graph_name.into(),
        }
    }

    #[inline]
    pub fn into_owned(self) -> Triple {
        Triple {
            subject: self.subject.into_owned(),
            predicate: self.predicate.into_owned(),
            object: self.object.into_owned(),
        }
    }
}

impl fmt::Display for TripleRef<'_> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {}", self.subject, self.predicate, self.object)
    }
}

impl<'a> From<&'a Triple> for TripleRef<'a> {
    #[inline]
    fn from(triple: &'a Triple) -> Self {
        triple.as_ref()
    }
}

impl<'a> From<TripleRef<'a>> for Triple {
    #[inline]
    fn from(triple: TripleRef<'a>) -> Self {
        triple.into_owned()
    }
}

#[derive(Eq, PartialEq, Debug, Clone, Hash, Default)]
pub enum GraphName {
    NamedNode(NamedNode),
    BlankNode(BlankNode),
    #[default]
    DefaultGraph,
}

impl GraphName {
    #[inline]
    pub fn is_named_node(&self) -> bool {
        self.as_ref().is_named_node()
    }

    #[inline]
    pub fn is_blank_node(&self) -> bool {
        self.as_ref().is_blank_node()
    }

    #[inline]
    pub fn is_default_graph(&self) -> bool {
        self.as_ref().is_default_graph()
    }

    #[inline]
    pub fn as_ref(&self) -> GraphNameRef<'_> {
        match self {
            Self::NamedNode(node) => GraphNameRef::NamedNode(node.as_ref()),
            Self::BlankNode(node) => GraphNameRef::BlankNode(node.as_ref()),
            Self::DefaultGraph => GraphNameRef::DefaultGraph,
        }
    }
}

impl fmt::Display for GraphName {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl From<NamedNode> for GraphName {
    #[inline]
    fn from(node: NamedNode) -> Self {
        Self::NamedNode(node)
    }
}

impl From<NamedNodeRef<'_>> for GraphName {
    #[inline]
    fn from(node: NamedNodeRef<'_>) -> Self {
        node.into_owned().into()
    }
}

impl From<BlankNode> for GraphName {
    #[inline]
    fn from(node: BlankNode) -> Self {
        Self::BlankNode(node)
    }
}

impl From<BlankNodeRef<'_>> for GraphName {
    #[inline]
    fn from(node: BlankNodeRef<'_>) -> Self {
        node.into_owned().into()
    }
}

impl From<NamedOrBlankNode> for GraphName {
    #[inline]
    fn from(node: NamedOrBlankNode) -> Self {
        match node {
            NamedOrBlankNode::NamedNode(node) => node.into(),
            NamedOrBlankNode::BlankNode(node) => node.into(),
        }
    }
}

impl From<NamedOrBlankNodeRef<'_>> for GraphName {
    #[inline]
    fn from(node: NamedOrBlankNodeRef<'_>) -> Self {
        node.into_owned().into()
    }
}

#[derive(Eq, PartialEq, Debug, Clone, Copy, Hash, Default)]
pub enum GraphNameRef<'a> {
    NamedNode(NamedNodeRef<'a>),
    BlankNode(BlankNodeRef<'a>),
    #[default]
    DefaultGraph,
}

impl GraphNameRef<'_> {
    #[inline]
    pub fn is_named_node(&self) -> bool {
        matches!(self, Self::NamedNode(_))
    }

    #[inline]
    pub fn is_blank_node(&self) -> bool {
        matches!(self, Self::BlankNode(_))
    }

    #[inline]
    pub fn is_default_graph(&self) -> bool {
        matches!(self, Self::DefaultGraph)
    }

    #[inline]
    pub fn into_owned(self) -> GraphName {
        match self {
            Self::NamedNode(node) => GraphName::NamedNode(node.into_owned()),
            Self::BlankNode(node) => GraphName::BlankNode(node.into_owned()),
            Self::DefaultGraph => GraphName::DefaultGraph,
        }
    }
}

impl fmt::Display for GraphNameRef<'_> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NamedNode(node) => node.fmt(f),
            Self::BlankNode(node) => node.fmt(f),
            Self::DefaultGraph => f.write_str("DEFAULT"),
        }
    }
}

impl<'a> From<NamedNodeRef<'a>> for GraphNameRef<'a> {
    #[inline]
    fn from(node: NamedNodeRef<'a>) -> Self {
        Self::NamedNode(node)
    }
}

impl<'a> From<&'a NamedNode> for GraphNameRef<'a> {
    #[inline]
    fn from(node: &'a NamedNode) -> Self {
        node.as_ref().into()
    }
}

impl<'a> From<BlankNodeRef<'a>> for GraphNameRef<'a> {
    #[inline]
    fn from(node: BlankNodeRef<'a>) -> Self {
        Self::BlankNode(node)
    }
}

impl<'a> From<&'a BlankNode> for GraphNameRef<'a> {
    #[inline]
    fn from(node: &'a BlankNode) -> Self {
        node.as_ref().into()
    }
}

impl<'a> From<NamedOrBlankNodeRef<'a>> for GraphNameRef<'a> {
    #[inline]
    fn from(node: NamedOrBlankNodeRef<'a>) -> Self {
        match node {
            NamedOrBlankNodeRef::NamedNode(node) => node.into(),
            NamedOrBlankNodeRef::BlankNode(node) => node.into(),
        }
    }
}

impl<'a> From<&'a NamedOrBlankNode> for GraphNameRef<'a> {
    #[inline]
    fn from(node: &'a NamedOrBlankNode) -> Self {
        node.as_ref().into()
    }
}

impl<'a> From<&'a GraphName> for GraphNameRef<'a> {
    #[inline]
    fn from(node: &'a GraphName) -> Self {
        node.as_ref()
    }
}

impl<'a> From<GraphNameRef<'a>> for GraphName {
    #[inline]
    fn from(node: GraphNameRef<'a>) -> Self {
        node.into_owned()
    }
}

#[derive(Eq, PartialEq, Debug, Clone, Hash)]
pub struct Quad {
    pub subject: NamedOrBlankNode,

    pub predicate: NamedNode,

    pub object: Term,

    pub graph_name: GraphName,
}

impl Quad {
    #[inline]
    pub fn new(
        subject: impl Into<NamedOrBlankNode>,
        predicate: impl Into<NamedNode>,
        object: impl Into<Term>,
        graph_name: impl Into<GraphName>,
    ) -> Self {
        Self {
            subject: subject.into(),
            predicate: predicate.into(),
            object: object.into(),
            graph_name: graph_name.into(),
        }
    }

    #[inline]
    pub fn as_ref(&self) -> QuadRef<'_> {
        QuadRef {
            subject: self.subject.as_ref(),
            predicate: self.predicate.as_ref(),
            object: self.object.as_ref(),
            graph_name: self.graph_name.as_ref(),
        }
    }
}

impl fmt::Display for Quad {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl From<Quad> for Triple {
    #[inline]
    fn from(quad: Quad) -> Self {
        Self {
            subject: quad.subject,
            predicate: quad.predicate,
            object: quad.object,
        }
    }
}

#[derive(Eq, PartialEq, Debug, Clone, Copy, Hash)]
pub struct QuadRef<'a> {
    pub subject: NamedOrBlankNodeRef<'a>,

    pub predicate: NamedNodeRef<'a>,

    pub object: TermRef<'a>,

    pub graph_name: GraphNameRef<'a>,
}

impl<'a> QuadRef<'a> {
    #[inline]
    pub fn new(
        subject: impl Into<NamedOrBlankNodeRef<'a>>,
        predicate: impl Into<NamedNodeRef<'a>>,
        object: impl Into<TermRef<'a>>,
        graph_name: impl Into<GraphNameRef<'a>>,
    ) -> Self {
        Self {
            subject: subject.into(),
            predicate: predicate.into(),
            object: object.into(),
            graph_name: graph_name.into(),
        }
    }

    #[inline]
    pub fn into_owned(self) -> Quad {
        Quad {
            subject: self.subject.into_owned(),
            predicate: self.predicate.into_owned(),
            object: self.object.into_owned(),
            graph_name: self.graph_name.into_owned(),
        }
    }
}

impl fmt::Display for QuadRef<'_> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.graph_name.is_default_graph() {
            write!(f, "{} {} {}", self.subject, self.predicate, self.object)
        } else {
            write!(
                f,
                "{} {} {} {}",
                self.subject, self.predicate, self.object, self.graph_name
            )
        }
    }
}

impl<'a> From<QuadRef<'a>> for TripleRef<'a> {
    #[inline]
    fn from(quad: QuadRef<'a>) -> Self {
        Self {
            subject: quad.subject,
            predicate: quad.predicate,
            object: quad.object,
        }
    }
}

impl<'a> From<&'a Quad> for QuadRef<'a> {
    #[inline]
    fn from(quad: &'a Quad) -> Self {
        quad.as_ref()
    }
}

impl<'a> From<QuadRef<'a>> for Quad {
    #[inline]
    fn from(quad: QuadRef<'a>) -> Self {
        quad.into_owned()
    }
}

#[derive(Debug, Clone)]
pub struct TryFromTermError {
    term: Term,
    target: &'static str,
}

impl fmt::Display for TryFromTermError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} can not be converted to a {}", self.term, self.target)
    }
}

impl std::error::Error for TryFromTermError {}

impl TryFromTermError {
    #[inline]
    pub fn into_term(self) -> Term {
        self.term
    }
}
