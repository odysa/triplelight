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

import static org.junit.jupiter.api.Assertions.*;

/** arrow-java imports the stream returned by {@link Database#queryArrow}; the cells match {@link Database#query}. */
class ArrowStreamTest {
    static final String XSD = "http://www.w3.org/2001/XMLSchema#";
    static final long STREAM_BYTES = 5 * 8; // struct ArrowArrayStream: four callbacks and private_data

    record Imported(List<String> names, List<String> rows, List<Integer> batches) {}

    static String cell(StructVector s, int i) {
        if (s.isNull(i)) return "-";
        String kind = new String(((VarCharVector) s.getChild("kind")).get(i), StandardCharsets.UTF_8);
        String value = new String(((LargeVarCharVector) s.getChild("value")).get(i), StandardCharsets.UTF_8);
        VarCharVector dt = (VarCharVector) s.getChild("datatype"), lang = (VarCharVector) s.getChild("language");
        String out = kind + "|" + value;
        if (!dt.isNull(i)) out += "|^" + new String(dt.get(i), StandardCharsets.UTF_8);
        if (!lang.isNull(i)) out += "|@" + new String(lang.get(i), StandardCharsets.UTF_8);
        return out;
    }

    static String viewCell(Term t) {
        return switch (t) {
            case null -> "-";
            case Term.Iri i -> "iri|" + i.value();
            case Term.BlankNode b -> "bnode|" + b.value();
            case Term.Literal l -> "literal|" + l.value() + "|^" + l.datatype() + (l.language().isEmpty() ? "" : "|@" + l.language());
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
                List<String> rows = new ArrayList<>();
                List<Integer> batches = new ArrayList<>();
                while (reader.loadNextBatch()) {
                    batches.add(root.getRowCount());
                    for (int r = 0; r < root.getRowCount(); r++) {
                        String[] parts = new String[root.getFieldVectors().size()];
                        for (int c = 0; c < parts.length; c++) parts[c] = cell((StructVector) root.getVector(c), r);
                        rows.add(String.join(" ", parts));
                    }
                }
                return new Imported(names, rows, batches);
            }
        }
    }

    static List<String> viaView(Database db, String q) {
        List<String> rows = new ArrayList<>();
        for (Term[] row : db.query(q).rows()) rows.add(String.join(" ", Arrays.stream(row).map(ArrowStreamTest::viewCell).toList()));
        return rows;
    }

    @Test
    void typedTermsMatchView() throws Exception {
        try (Database db = Database.inMemory()) {
            db.update("INSERT DATA { <urn:a> <urn:p> <urn:iri> . <urn:a> <urn:p> \"urn:iri\" . <urn:a> <urn:p> \"\" ."
                    + " <urn:a> <urn:p> \"hi\"@en . <urn:a> <urn:p> \"01\"^^<" + XSD + "int> . <urn:a> <urn:p> _:b }");
            String q = "SELECT ?o ?z ?len WHERE { <urn:a> <urn:p> ?o OPTIONAL { ?o <urn:none> ?z }"
                    + " BIND(IF(isLiteral(?o), STRLEN(STR(?o)), 0) AS ?len) } ORDER BY STR(?o)";
            Imported got = viaStream(db, q);
            assertEquals(db.query(q).variables(), got.names());
            assertEquals(viaView(db, q), got.rows());
            assertEquals(6, got.rows().size());
        }
    }

    @Test
    void batchBoundaries() throws Exception {
        for (int n : new int[] {8191, 8192, 8193}) {
            try (Database db = Database.inMemory()) {
                StringBuilder b = new StringBuilder("INSERT DATA {");
                for (int i = 0; i < n; i++) b.append(" <urn:s").append(i).append("> <urn:p> \"").append(i).append("\"@en .");
                db.update(b.append(" }").toString());
                String q = "SELECT ?s ?o ?z WHERE { ?s <urn:p> ?o OPTIONAL { ?s <urn:none> ?z } } ORDER BY ?s";
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
            db.update("INSERT DATA { <urn:a> <urn:p> 1 . <urn:b> <urn:p> 2 }");
            Imported empty = viaStream(db, "SELECT ?s WHERE { ?s <urn:none> ?o }");
            assertEquals(List.of("s"), empty.names());
            assertTrue(empty.rows().isEmpty());
            Imported zero = viaStream(db, "SELECT * WHERE { <urn:a> <urn:p> 1 }");
            assertTrue(zero.names().isEmpty());
            assertEquals(1, zero.batches().stream().mapToInt(Integer::intValue).sum(), "zero-column cardinality");
            assertEquals(0, viaStream(db, "SELECT * WHERE { <urn:a> <urn:p> 2 }").batches().stream().mapToInt(Integer::intValue).sum());
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
                assertEquals("literal|x|^" + XSD + "string", cell((StructVector) root.getVector(1), 0));
                assertFalse(reader.loadNextBatch());
            }
        }
    }
}
