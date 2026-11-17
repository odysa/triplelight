// Uses the BSBM tool's own parameter pool; does not execute database queries.
import benchmark.testdriver.LocalSPARQLParameterPool;
import benchmark.testdriver.Query;
import benchmark.testdriver.QueryMix;
import java.io.File;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.stream.Collectors;

public final class ExportBiQueries {
    public static void main(String[] args) throws Exception {
        Path templates = Path.of(args[0]);
        Path resources = Path.of(args[1]);
        Path output = Path.of(args[2]);
        long seed = Long.parseLong(args[3]);
        int mixes = Integer.parseInt(args[4]);
        int warmups = Integer.parseInt(args[5]);
        Query[] queries = new Query[8];
        for (int i = 1; i <= 8; i++) {
            queries[i-1] = new Query(templates.resolve("query"+i+".txt").toFile(),
                templates.resolve("query"+i+"desc.txt").toFile(), "%");
        }
        List<Integer> order = Files.readAllLines(templates.resolve("querymix.txt")).stream()
            .map(String::strip).filter(s -> !s.isEmpty()).map(Integer::parseInt).collect(Collectors.toList());
        QueryMix mix = new QueryMix(queries, order.toArray(new Integer[0]));
        LocalSPARQLParameterPool pool = new LocalSPARQLParameterPool(resources.toFile(), seed);
        Files.createDirectories(output);
        try (PrintWriter index = new PrintWriter(output.resolve("index.tsv").toFile(), StandardCharsets.UTF_8)) {
            index.println("mix\tposition\ttemplate\twarmup\tfile");
            for (int m = 0; m < mixes; m++) {
                mix.setRun(m - warmups);
                int position = 0;
                while (mix.hasNext()) {
                    Query q = mix.getNext();
                    q.setParameters(pool.getParametersForQuery(q));
                    String name = String.format("m%03d_p%02d_q%d.rq", m, position, q.getNr());
                    Files.writeString(output.resolve(name), q.getQueryString(), StandardCharsets.UTF_8);
                    index.printf("%d\t%d\t%d\t%b\t%s%n", m, position++, q.getNr(), m < warmups, name);
                    mix.setCurrent(0, -1.0);
                }
                mix.finishRun();
            }
        }
    }
}
