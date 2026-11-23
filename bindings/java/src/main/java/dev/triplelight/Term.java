package dev.triplelight;

/** An RDF term: an IRI, a blank node, a literal or (RDF 1.2, builds with the rdf-12 feature) a triple term. */
public sealed interface Term permits Term.Iri, Term.BlankNode, Term.Literal, Term.Triple {
    /** The IRI, blank node label or literal lexical form. */
    String value();

    record Iri(String value) implements Term {}

    record BlankNode(String value) implements Term {}

    /**
     * A literal. {@code datatype} is always set; {@code language} is empty unless the literal is language-tagged
     * (then {@code datatype} is {@code rdf:langString}).
     */
    record Literal(String value, String language, String datatype, String direction) implements Term {
        /** A literal without a base direction. */
        public Literal(String value, String language, String datatype) { this(value, language, datatype, ""); }
    }

    /**
     * An RDF 1.2 triple term: subject, predicate and object are terms (which may be triple terms themselves).
     * {@link #value()} is empty; {@code direction} on {@link Literal} is {@code "ltr"}, {@code "rtl"} or empty.
     */
    record Triple(Term subject, Term predicate, Term object) implements Term {
        @Override public String value() { return ""; }
    }
}
