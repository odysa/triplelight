// Command overhead times Go DB.Query (C ABI call plus decoding into Go values) on an in-memory database.
//
//	go run ./cmd/overhead <data.nt> <reps>
package main

import (
	"fmt"
	"os"
	"sort"
	"strconv"
	"time"

	triplelight "github.com/odysa/triplelight/bindings/go"
)

var queries = [][2]string{
	{"lubm_q1", "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> PREFIX ub: <http://swat.cse.lehigh.edu/onto/univ-bench.owl#> SELECT ?X WHERE { ?X rdf:type ub:GraduateStudent . ?X ub:takesCourse <http://www.Department0.University0.edu/GraduateCourse0> }"},
	{"lubm_q14", "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> PREFIX ub: <http://swat.cse.lehigh.edu/onto/univ-bench.owl#> SELECT ?X WHERE { ?X rdf:type ub:UndergraduateStudent }"},
	{"wide_200k", "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 200000"},
	{"group_by_p", "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p"},
}

func main() {
	reps, _ := strconv.Atoi(os.Args[2])
	db, err := triplelight.OpenInMemory()
	if err != nil {
		panic(err)
	}
	defer db.Close()
	if _, err := db.Load(os.Args[1]); err != nil {
		panic(err)
	}
	for !db.StatsReady() {
		time.Sleep(20 * time.Millisecond)
	}
	for _, q := range queries {
		var rows, cols int
		run := func() {
			r, err := db.Query(q[1])
			if err != nil {
				panic(err)
			}
			rows, cols = len(r.Rows), len(r.Vars)
		}
		for i := 0; i < 3; i++ {
			run()
		}
		t := make([]float64, reps)
		for i := range t {
			s := time.Now()
			run()
			t[i] = float64(time.Since(s).Nanoseconds()) / 1e6
		}
		sort.Float64s(t)
		fmt.Printf("{\"lang\":\"go\",\"query\":\"%s\",\"rows\":%d,\"cols\":%d,\"ms\":%.3f}\n", q[0], rows, cols, t[len(t)/2])
	}
}
