package dev.triplelight;

import java.util.List;

/**
 * A fully decoded query result, independent of the database. For {@link Type#BINDINGS}, {@link #variables()} lists the
 * projection (also when there are no rows) and each row holds one term per variable, {@code null} when unbound. For
 * {@link Type#TRIPLES} each row is {@code [subject, predicate, object]}. Rows share {@link Term} instances: each
 * distinct term is decoded once.
 */
public record Result(Type type, List<String> variables, List<Term[]> rows, boolean booleanValue) {
    public enum Type { BINDINGS, BOOLEAN, TRIPLES }
}
