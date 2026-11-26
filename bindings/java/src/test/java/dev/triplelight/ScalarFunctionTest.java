package dev.triplelight;

import org.junit.jupiter.api.Test;

import java.io.IOException;
import java.util.concurrent.Executors;

import static org.junit.jupiter.api.Assertions.*;

class ScalarFunctionTest {
    private static final String ECHO = "SELECT ?o (<urn:java:echo>(?o) AS ?echo) WHERE { <urn:s> <urn:p> ?o }";

    @Test
    void copiesTermsAndSupportsConcurrentCalls() throws Exception {
        Database db = Database.inMemory();
        try (db; var workers = Executors.newFixedThreadPool(2)) {
            db.update("INSERT DATA { <urn:s> <urn:p> <urn:iri>, _:b, \"01\"^^<" + DatabaseTest.XSD + "int>,"
                    + " \"hi\"@en, \"a\\u0000b\\t\\\"\\\\\" }");
            db.registerFunction("urn:java:echo", 1, true, args -> args[0]);
            assertThrows(TripleLightException.class,
                    () -> db.registerFunction("urn:java:echo", 1, false, args -> null));
            assertThrows(TripleLightException.class,
                    () -> db.registerFunction("relative", 1, false, args -> null));
            var first = workers.submit(() -> db.query(ECHO));
            var second = workers.submit(() -> db.query(ECHO));
            for (Result result : new Result[]{first.get(), second.get()}) {
                assertEquals(5, result.rows().size());
                for (Term[] row : result.rows()) assertEquals(row[0], row[1]);
            }
        }
        assertEquals(TripleLightException.Kind.CLOSED, assertThrows(TripleLightException.class,
                () -> db.registerFunction("urn:java:echo", 1, true, args -> args[0])).kind());
    }

    @Test
    void convertsCallbackFailuresToExpressionErrors() {
        try (Database db = Database.inMemory()) {
            db.registerFunction("urn:java:throws", 0, false, args -> { throw new IOException("callback failed"); });
            db.registerFunction("urn:java:nil", 0, false, args -> null);
            db.registerFunction("urn:java:bad", 0, false, args -> new Term.Iri("not an IRI"));
            db.registerFunction("urn:java:bad-lang-datatype", 0, false,
                    args -> new Term.Literal("1", "en", DatabaseTest.XSD + "int"));
            db.registerFunction("urn:java:zero", 0, false, args -> new Term.Iri("urn:ok"));
            Term[] row = db.query("SELECT (<urn:java:throws>() AS ?a) (<urn:java:nil>() AS ?b)"
                    + " (<urn:java:bad>() AS ?c) (<urn:java:zero>(1) AS ?d)"
                    + " (<urn:java:bad-lang-datatype>() AS ?e) (<urn:java:zero>() AS ?f) WHERE {}")
                    .rows().getFirst();
            assertArrayEquals(new Term[]{null, null, null, null, null, new Term.Iri("urn:ok")}, row);
        }
    }

    @Test
    void copiesRdf12TripleTermsAndDirection() {
        try (Database db = Database.inMemory()) {
            try {
                db.update("INSERT DATA { <urn:s> <urn:p> <<( <urn:a> <urn:p> \"שלום\"@he--rtl )>> }");
            } catch (TripleLightException error) {
                if (System.getenv("TRIPLELIGHT_EXPECT_RDF12") != null) throw error;
                org.junit.jupiter.api.Assumptions.assumeTrue(false, "requires RDF 1.2 build");
            }
            db.registerFunction("urn:java:echo", null, false, args -> args[0]);
            Term[] row = db.query(ECHO).rows().getFirst();
            assertInstanceOf(Term.Triple.class, row[0]);
            assertEquals(row[0], row[1]);
        }
    }
}
