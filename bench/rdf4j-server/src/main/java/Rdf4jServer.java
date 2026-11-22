import com.sun.net.httpserver.HttpServer;
import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.util.concurrent.Executors;
import org.eclipse.rdf4j.query.*;
import org.eclipse.rdf4j.query.resultio.sparqljson.SPARQLResultsJSONWriter;
import org.eclipse.rdf4j.repository.sail.SailRepository;
import org.eclipse.rdf4j.rio.RDFFormat;
import org.eclipse.rdf4j.sail.memory.MemoryStore;
import org.eclipse.rdf4j.sail.nativerdf.NativeStore;

/** Minimal SPARQL protocol endpoint over RDF4J. args: native|memory <dataDir> <file.nt|-> <port>.
 *  Port 0 loads the file, prints the created indexes and exits (load-only mode).
 *  Env: RDF4J_INDEXES (native store index spec, default spoc,posc,ospc), RDF4J_THREADS (HTTP worker threads, default 4). */
public class Rdf4jServer {
  public static void main(String[] a) throws Exception {
    var sail = a[0].equals("native") ? new NativeStore(new File(a[1]), System.getenv().getOrDefault("RDF4J_INDEXES", "spoc,posc,ospc")) : new MemoryStore();
    var repo = new SailRepository(sail);
    repo.init();
    if (!a[2].equals("-")) {
      long t = System.nanoTime();
      try (var c = repo.getConnection()) { c.begin(); c.add(new File(a[2]), RDFFormat.NTRIPLES); c.commit(); }
      System.err.printf("loaded in %.2fs%n", (System.nanoTime() - t) / 1e9);
    }
    if (a[3].equals("0")) { repo.shutDown(); System.err.println("indexes: " + System.getenv().getOrDefault("RDF4J_INDEXES", "spoc,posc,ospc")); return; }
    var srv = HttpServer.create(new InetSocketAddress("127.0.0.1", Integer.parseInt(a[3])), 64);
    srv.createContext("/sparql", ex -> {
      String body = new String(ex.getRequestBody().readAllBytes(), StandardCharsets.UTF_8);
      String q = null;
      for (String kv : body.split("&")) if (kv.startsWith("query=")) q = URLDecoder.decode(kv.substring(6), StandardCharsets.UTF_8);
      var out = new ByteArrayOutputStream();
      int code = 200;
      try (var c = repo.getConnection()) {
        var pq = c.prepareQuery(QueryLanguage.SPARQL, q);
        if (pq instanceof TupleQuery tq) tq.evaluate(new SPARQLResultsJSONWriter(out));
        else if (pq instanceof BooleanQuery bq) out.write(("{\"head\":{},\"boolean\":" + bq.evaluate() + "}").getBytes());
      } catch (Exception e) { code = 400; out.reset(); out.write(String.valueOf(e).getBytes()); }
      ex.getResponseHeaders().add("Content-Type", "application/sparql-results+json");
      ex.sendResponseHeaders(code, out.size());
      try (var os = ex.getResponseBody()) { out.writeTo(os); }
    });
    srv.setExecutor(Executors.newFixedThreadPool(Integer.parseInt(System.getenv().getOrDefault("RDF4J_THREADS", "4"))));
    srv.start();
    System.err.println("serving on " + a[3]);
  }
}
