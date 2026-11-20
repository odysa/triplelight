package dev.triplelight;

import org.apache.arrow.c.ArrowArrayStream;
import org.apache.arrow.c.Data;
import org.apache.arrow.memory.BufferAllocator;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.LargeVarCharVector;
import org.apache.arrow.vector.VarCharVector;
import org.apache.arrow.vector.VectorSchemaRoot;
import org.apache.arrow.vector.complex.StructVector;
import org.apache.arrow.vector.ipc.ArrowReader;
import org.apache.arrow.vector.types.pojo.ArrowType;
import org.apache.arrow.vector.types.pojo.Field;
import org.junit.jupiter.api.Test;

import java.lang.foreign.MemorySegment;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

/** arrow-java imports the stream returned by {@link Database#queryArrow}; the cells match {@link Database#query}. */
class ArrowStreamTest {
    static final String XSD = "http://www.w3.org/2001/XMLSchema#";
    static final long STREAM_BYTES = 5 * 8; // struct ArrowArrayStream: four callbacks and private_data

    /** One cell; {@code UNBOUND} for an unbound cell, null datatype/language when absent. */
    record Cell(boolean bound, String kind, String value, String datatype, String language) {}
    static final Cell UNBOUND = new Cell(false, null, null, null, null);

    record Imported(List<String> names, List<List<Cell>> rows, List<Integer> batches) {}

    static String opt(VarCharVector v, int i) { return v.isNull(i) ? null : new String(v.get(i), StandardCharsets.UTF_8); }

    static Cell cell(StructVector s, int i) {
        if (s.isNull(i)) return UNBOUND;
        return new Cell(true, opt((VarCharVector) s.getChild("kind"), i),
                new String(((LargeVarCharVector) s.getChild("value")).get(i), StandardCharsets.UTF_8),
                opt((VarCharVector) s.getChild("datatype"), i), opt((VarCharVector) s.getChild("language"), i));
    }

    static Cell viewCell(Term t) {
        return switch (t) {
            case null -> UNBOUND;
            case Term.Iri i -> new Cell(true, "iri", i.value(), null, null);
            case Term.BlankNode b -> new Cell(true, "bnode", b.value(), null, null);
            case Term.Literal l -> new Cell(true, "literal", l.value(), l.datatype(), l.language().isEmpty() ? null : l.language());
        };
    }

    static Imported viaStream(Database db, String q) throws Exception {
        try (BufferAllocator alloc = new RootAllocator(); ArrowArrayStream stream = ArrowArrayStream.allocateNew(alloc)) {
            db.queryArrow(q, stream.memoryAddress());
            try (ArrowReader reader = Data.importArrayStream(alloc, stream)) {
                VectorSchemaRoot root = reader.getVectorSchemaRoot();
                List<String> names = new ArrayList<>();
                for (Field f : root.getSchema().getFields()) {
                    assertTrue(f.isNullable(), f.getName());
                    assertInstanceOf(ArrowType.Struct.class, f.getType(), f.getName());
                    names.add(f.getName());
                }
                List<List<Cell>> rows = new ArrayList<>();
                List<Integer> batches = new ArrayList<>();
                while (reader.loadNextBatch()) {
                    batches.add(root.getRowCount());
                    assertEquals(names.size(), root.getFieldVectors().size());
                    for (int r = 0; r < root.getRowCount(); r++) {
                        List<Cell> row = new ArrayList<>();
                        for (int c = 0; c < names.size(); c++) row.add(cell((StructVector) root.getVector(c), r));
                        rows.add(row);
                    }
                }
                return new Imported(names, rows, batches);
            }
        }
    }

    static List<List<Cell>> viaView(Database db, String q) {
        List<List<Cell>> rows = new ArrayList<>();
        for (Term[] row : db.query(q).rows()) rows.add(Arrays.stream(row).map(ArrowStreamTest::viewCell).toList());
        return rows;
    }

    @Test
    void typedTermsMatchView() throws Exception {
        try (Database db = Database.inMemory()) {
            db.update("INSERT DATA { <urn:a> <urn:p> <urn:iri> . <urn:a> <urn:p> \"urn:iri\" . <urn:a> <urn:p> \"\" ."
                    + " <urn:a> <urn:p> \"hi\"@en . <urn:a> <urn:p> \"01\"^^<" + XSD + "int> . <urn:a> <urn:p> _:b ."
                    + " <urn:a> <urn:p> \"a|b ^c @d - e\" }");
            String q = "SELECT ?o ?z ?len WHERE { <urn:a> <urn:p> ?o OPTIONAL { ?o <urn:none> ?z }"
                    + " BIND(IF(isLiteral(?o), STRLEN(STR(?o)), 0) AS ?len) } ORDER BY STR(?o)";
            Imported got = viaStream(db, q);
            assertEquals(db.query(q).variables(), got.names());
            assertEquals(viaView(db, q), got.rows());
            assertEquals(7, got.rows().size());
            assertTrue(got.rows().stream().anyMatch(r -> r.get(0).equals(new Cell(true, "literal", "a|b ^c @d - e", XSD + "string", null))
                    && r.get(1).equals(UNBOUND)), "delimiter-bearing literal");
        }
    }

    @Test
    void batchBoundaries() throws Exception {
        for (int n : new int[] {8191, 8192, 8193, 16387}) {
            try (Database db = Database.inMemory()) {
                StringBuilder b = new StringBuilder("INSERT DATA {");
                for (int i = 0; i < n; i++) {
                    // Mixed across the boundary: language-tagged, empty, noncanonical integers; every 5th row binds ?x.
                    String o = switch (i % 7) { case 0 -> "\"t" + i + "\"@en"; case 3 -> "\"\""; default -> "\"0" + i + "\"^^<" + XSD + "int>"; };
                    String s = String.format("<urn:r%06d>", i);
                    b.append(' ').append(s).append(" <urn:p> ").append(o).append(" .");
                    if (i % 5 == 0) b.append(' ').append(s).append(" <urn:q> <urn:x").append(i).append("> .");
                }
                db.update(b.append(" }").toString());
                String q = "SELECT ?s ?o ?x WHERE { ?s <urn:p> ?o OPTIONAL { ?s <urn:q> ?x } } ORDER BY ?s";
                Imported got = viaStream(db, q);
                assertEquals(viaView(db, q), got.rows(), "n=" + n);
                int total = 0;
                for (int i = 0; i < got.batches().size(); i++) {
                    int s = got.batches().get(i);
                    assertTrue(s <= 8192 && (i == got.batches().size() - 1 || s == 8192), "batches " + got.batches());
                    total += s;
                }
                assertEquals(n, total);
            }
        }
    }

    @Test
    void zeroColumnsAndEmpty() throws Exception {
        try (Database db = Database.inMemory()) {
            db.update("INSERT DATA { <urn:a> <urn:p> 1 . <urn:a> <urn:p> 2 . <urn:a> <urn:p> 3 . <urn:b> <urn:p> 2 }");
            Imported empty = viaStream(db, "SELECT ?s WHERE { ?s <urn:none> ?o }");
            assertEquals(List.of("s"), empty.names());
            assertTrue(empty.rows().isEmpty());
            // No variables: the solution count survives as the batch row count.
            Map<String, Integer> cases = Map.of("SELECT * WHERE { <urn:a> <urn:p> 2 }", 1, "SELECT * WHERE { <urn:a> <urn:p> [] }", 3,
                    "SELECT * WHERE { <urn:a> <urn:p> [] . [] <urn:p> 2 }", 6, "SELECT * WHERE { <urn:a> <urn:p> 9 }", 0);
            for (var c : cases.entrySet()) {
                Imported got = viaStream(db, c.getKey());
                assertTrue(got.names().isEmpty() && db.query(c.getKey()).variables().isEmpty(), c.getKey());
                assertEquals(c.getValue(), got.rows().size(), c.getKey());
                assertEquals(c.getValue(), db.query(c.getKey()).rows().size(), c.getKey());
            }
        }
    }

    @Test
    void errorsLeaveStreamUntouchedAndStreamOutlivesClose() throws Exception {
        try (BufferAllocator alloc = new RootAllocator(); ArrowArrayStream stream = ArrowArrayStream.allocateNew(alloc)) {
            MemorySegment raw = MemorySegment.ofAddress(stream.memoryAddress()).reinterpret(STREAM_BYTES);
            Database db = Database.inMemory();
            db.update("INSERT DATA { <urn:a> <urn:p> \"x\" }");
            byte[] before = raw.toArray(java.lang.foreign.ValueLayout.JAVA_BYTE);
            for (String q : List.of("ASK { ?s ?p ?o }", "CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }", "SELECT WHERE {")) {
                TripleLightException e = assertThrows(TripleLightException.class, () -> db.queryArrow(q, stream.memoryAddress()));
                if (!q.startsWith("SELECT")) assertEquals(TripleLightException.Kind.UNSUPPORTED, e.kind(), q);
                assertArrayEquals(before, raw.toArray(java.lang.foreign.ValueLayout.JAVA_BYTE), q);
            }
            assertThrows(IllegalArgumentException.class, () -> db.queryArrow("SELECT * {}", 0));
            db.queryArrow("SELECT ?s ?o WHERE { ?s ?p ?o }", stream.memoryAddress());
            db.close();
            try (ArrowReader reader = Data.importArrayStream(alloc, stream)) {
                assertTrue(reader.loadNextBatch());
                VectorSchemaRoot root = reader.getVectorSchemaRoot();
                assertEquals(1, root.getRowCount());
                assertEquals(new Cell(true, "literal", "x", XSD + "string", null), cell((StructVector) root.getVector(1), 0));
                assertFalse(reader.loadNextBatch());
            }
        }
    }
}
