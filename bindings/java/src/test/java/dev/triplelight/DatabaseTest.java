package dev.triplelight;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.function.Function;
import java.util.stream.Collectors;

import static org.junit.jupiter.api.Assertions.*;

class DatabaseTest {
    static final String XSD = "http://www.w3.org/2001/XMLSchema#";

    @Test
    void typedTermsUnboundAndSharing() {
        try (Database db = Database.inMemory()) {
            assertEquals(4, db.update("INSERT DATA { <urn:a> <urn:p> \"01\"^^<" + XSD + "int> . <urn:a> <urn:p> \"hi\"@en ."
                    + " <urn:a> <urn:q> _:b . <urn:c> <urn:p> \"x\" }"));
            Result r = db.query("SELECT ?s ?o ?z WHERE { ?s <urn:p> ?o OPTIONAL { ?s <urn:none> ?z } } ORDER BY ?s ?o");
            assertEquals(Result.Type.BINDINGS, r.type());
            assertEquals(List.of("s", "o", "z"), r.variables());
            assertEquals(3, r.rows().size());
            r.rows().forEach(row -> assertNull(row[2]));
            assertSame(r.rows().get(0)[0], r.rows().get(1)[0], "distinct terms are shared across rows");
            Map<String, Term> objects = r.rows().stream().map(row -> row[1]).collect(Collectors.toMap(Term::value, Function.identity()));
            assertEquals(new Term.Literal("01", "", XSD + "int"), objects.get("01"));
            assertEquals(new Term.Literal("hi", "en", "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString"), objects.get("hi"));
            assertEquals(new Term.Literal("x", "", XSD + "string"), objects.get("x"));
            assertEquals(new Term.Iri("urn:a"), r.rows().get(0)[0]);
        }
    }

    @Test
    void emptyProjectionAskConstructAndNul() {
        try (Database db = Database.inMemory()) {
            db.update("INSERT DATA { <urn:a> <urn:q> _:b . <urn:a> <urn:n> \"a\\u0000b\" }");
            Result empty = db.query("SELECT ?x ?y WHERE { ?x <urn:nothing> ?y }");
            assertEquals(List.of("x", "y"), empty.variables());
            assertTrue(empty.rows().isEmpty());
            Result ask = db.query("ASK { <urn:a> <urn:q> ?b }");
            assertEquals(Result.Type.BOOLEAN, ask.type());
            assertTrue(ask.booleanValue());
            Result g = db.query("CONSTRUCT { ?s <urn:r> ?o } WHERE { ?s <urn:q> ?o }");
            assertEquals(Result.Type.TRIPLES, g.type());
            assertInstanceOf(Term.BlankNode.class, g.rows().get(0)[2]);
            assertEquals("a\u0000b", db.query("SELECT ?v WHERE { <urn:a> <urn:n> ?v }").rows().get(0)[0].value());
        }
    }

    @Test
    void errorsAndClose() {
        Database db = Database.inMemory();
        assertEquals(TripleLightException.Kind.SYNTAX, assertThrows(TripleLightException.class, () -> db.query("SELEC nope")).kind());
        assertEquals(TripleLightException.Kind.STORAGE, assertThrows(TripleLightException.class, () -> db.load(Path.of("/nonexistent/missing.nt"))).kind());
        db.close();
        db.close();
        assertEquals(TripleLightException.Kind.CLOSED, assertThrows(TripleLightException.class, () -> db.query("ASK {}")).kind());
    }

    @Test
    void persistentLockAndParity(@TempDir Path tmp) throws Exception {
        Path dir = tmp.resolve("db");
        Path nt = tmp.resolve("d.nt");
        Files.writeString(nt, "<urn:a> <urn:p> \"1\"^^<" + XSD + "integer> .\n<urn:b> <urn:p> \"2\"^^<" + XSD + "integer> .\n");
        try (Database disk = Database.open(dir); Database mem = Database.inMemory()) {
            assertEquals(TripleLightException.Kind.LOCKED, assertThrows(TripleLightException.class, () -> Database.open(dir)).kind());
            for (Database d : List.of(disk, mem)) {
                assertEquals(2, d.load(nt));
                assertEquals("3", d.query("SELECT (SUM(?o) AS ?n) WHERE { ?s ?p ?o }").rows().get(0)[0].value());
            }
        }
        try (Database again = Database.open(dir)) {
            assertEquals("2", again.query("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }").rows().get(0)[0].value());
        }
    }

    @Test
    void concurrentQueriesThenClose() throws Exception {
        Database db = Database.inMemory();
        db.update("INSERT DATA { <urn:a> <urn:p> 1 }");
        ExecutorService pool = Executors.newFixedThreadPool(8);
        List<Future<?>> fs = new java.util.ArrayList<>();
        for (int i = 0; i < 8; i++) {
            fs.add(pool.submit(() -> {
                for (int j = 0; j < 200; j++) {
                    try {
                        db.query("SELECT * WHERE { ?s ?p ?o }");
                    } catch (TripleLightException e) {
                        assertEquals(TripleLightException.Kind.CLOSED, e.kind());
                        return;
                    }
                }
            }));
        }
        db.close();
        for (Future<?> f : fs) f.get();
        pool.shutdown();
    }
}
