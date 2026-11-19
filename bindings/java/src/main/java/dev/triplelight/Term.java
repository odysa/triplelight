package dev.triplelight;

/** An RDF term: an IRI, a blank node or a literal. */
public sealed interface Term permits Term.Iri, Term.BlankNode, Term.Literal {
    /** The IRI, blank node label or literal lexical form. */
    String value();

    record Iri(String value) implements Term {}

    record BlankNode(String value) implements Term {}

    /**
     * A literal. {@code datatype} is always set; {@code language} is empty unless the literal is language-tagged
     * (then {@code datatype} is {@code rdf:langString}).
     */
    record Literal(String value, String language, String datatype) implements Term {}
}
