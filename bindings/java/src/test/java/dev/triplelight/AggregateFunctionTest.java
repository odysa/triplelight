package dev.triplelight;

import org.junit.jupiter.api.Test;

import java.io.IOException;
import java.util.concurrent.Executors;

import static org.junit.jupiter.api.Assertions.*;

class AggregateFunctionTest {
    private static class Count implements Database.AggregateState {
        private int total;

        @Override
        public void step(Term value) throws Exception {
            total++;
        }

        @Override
        public Term finish() throws Exception {
            return new Term.Literal(Integer.toString(total), "", DatabaseTest.XSD + "integer");
        }
    }

    @Test
    void usesIndependentStateForConcurrentQueriesAndGroups() throws Exception {
        try (Database db = Database.inMemory(); var workers = Executors.newFixedThreadPool(2)) {
            db.registerAggregateFunction("urn:java:count", Count::new);
            assertThrows(TripleLightException.class,
                    () -> db.registerAggregateFunction("urn:java:count", Count::new));
            var query = """
                    SELECT ?g (<urn:java:count>(DISTINCT ?v) AS ?n) WHERE {
                        VALUES (?g ?v) { (1 "x") (1 "x") (1 UNDEF) (2 "y") (2 "z") }
                    } GROUP BY ?g ORDER BY ?g
                    """;
            var first = workers.submit(() -> db.query(query));
            var second = workers.submit(() -> db.query(query));
            var expected = db.query(query.replace("<urn:java:count>", "COUNT")).rows();
            for (var actual : new Result[]{first.get(), second.get()}) {
                assertEquals(expected.size(), actual.rows().size());
                for (int i = 0; i < expected.size(); i++) assertArrayEquals(expected.get(i), actual.rows().get(i));
            }
            var empty = "SELECT (<urn:java:count>(?v) AS ?n) WHERE { FILTER(false) }";
            assertArrayEquals(db.query(empty.replace("<urn:java:count>", "COUNT")).rows().getFirst(),
                    db.query(empty).rows().getFirst());
        }
    }

    @Test
    void containsExceptionsInEachCallbackPhase() {
        try (Database db = Database.inMemory()) {
            db.registerAggregateFunction("urn:java:create", () -> { throw new IOException("create"); });
            db.registerAggregateFunction("urn:java:step", () -> new Count() {
                @Override public void step(Term value) throws Exception { throw new IOException("step"); }
            });
            db.registerAggregateFunction("urn:java:finish", () -> new Count() {
                @Override public Term finish() throws Exception { throw new IOException("finish"); }
            });
            var result = db.query("""
                    SELECT (<urn:java:create>(?v) AS ?a) (<urn:java:step>(?v) AS ?b)
                        (<urn:java:finish>(?v) AS ?c) WHERE { VALUES ?v { "x" } }
                    """);
            assertArrayEquals(new Term[]{null, null, null}, result.rows().getFirst());
        }
    }
}
