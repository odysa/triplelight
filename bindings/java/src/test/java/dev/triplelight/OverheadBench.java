package dev.triplelight;

import java.nio.file.Path;
import java.util.Arrays;

/** Times Java Database.query (FFM call plus decoding into Java objects) on an in-memory database. */
public final class OverheadBench {
    static final String[][] QUERIES = {
        {"lubm_q1", "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> PREFIX ub: <http://swat.cse.lehigh.edu/onto/univ-bench.owl#> SELECT ?X WHERE { ?X rdf:type ub:GraduateStudent . ?X ub:takesCourse <http://www.Department0.University0.edu/GraduateCourse0> }"},
        {"lubm_q14", "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> PREFIX ub: <http://swat.cse.lehigh.edu/onto/univ-bench.owl#> SELECT ?X WHERE { ?X rdf:type ub:UndergraduateStudent }"},
        {"wide_200k", "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 200000"},
        {"group_by_p", "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p"},
    };

    public static void main(String[] args) throws Exception {
        int reps = Integer.parseInt(args[1]);
        try (Database db = Database.inMemory()) {
            db.load(Path.of(args[0]));
            while (!db.statsReady()) Thread.sleep(20);
            for (String[] q : QUERIES) {
                Result r = null;
                int warm = Integer.parseInt(System.getProperty("warmup", "10")); for (int i = 0; i < warm; i++) r = db.query(q[1]); // JIT warm-up
                double[] t = new double[reps];
                for (int i = 0; i < reps; i++) {
                    long s = System.nanoTime();
                    r = db.query(q[1]);
                    t[i] = (System.nanoTime() - s) / 1e6;
                }
                Arrays.sort(t);
                System.out.printf("{\"lang\":\"java\",\"query\":\"%s\",\"rows\":%d,\"cols\":%d,\"ms\":%.3f}%n",
                        q[0], r.rows().size(), r.variables().size(), t[reps / 2]);
            }
        }
    }
}
