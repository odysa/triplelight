# Bounded decimal v3: qualification passes

Candidate `9a2f49b652a73466873cacd0a9205098e27d15f6` passes all 29 retained control requests against its stated bounded numeric contract, followed by all 60 BI100M queries under the unchanged primary, COUNT-compatible and value-compatible policies. Every BI result also matches the previous baseline's complete RDF-term bag and sequence. Sources, executable and immutable disk snapshot remain unchanged, and the recorded owned processes have exited. The exact owned lock was released.

This supports the scoped decimal implementation. It does not establish arbitrary-precision decimal support or a performance gain. The measured BI100M performance table remains pinned to ef7a778.

## Numeric contract and controls

The implementation uses a 96-bit decimal mantissa with at most 28 fractional digits. Addition, subtraction, multiplication and terminating division must fit exactly after stripping trailing zeros, or yield a SPARQL error. Non-terminating decimal division may round with implementation-defined precision. Integers remain limited to i64. The author now computes decimal operations with exact integer mantissas before checking representability.

All prior counterexamples were retained. ROUND at the decimal maximum no longer panics. Decimal-to-float operations and comparisons convert operands to binary32 first. Scale-29 literals, MAX plus 0.1 in either operand order, the product requiring an unrepresentable .01 fraction, and the terminating quotient below the scale limit all return errors rather than silent rounded values.

The replay preserves the actual independent Jena responses from the previous attempt, hashes them before reuse and verifies the original query manifest. Of 29 requests, 16 match Jena's RDF terms exactly, 12 retain explicitly classified differences and one has an unscored Jena oracle exception. The controls include duplicate scale-29 queries and are not 29 distinct texts. Contract acceptance is recorded separately from exact Jena comparison:

- Out-of-range decimal values/results and integer overflow produce unbound expressions under the declared bounded contract, where Jena can return arbitrary-precision values.
- Repeating division 1/3 differs in decimal precision; the candidate is checked against the exact rational within half a unit at the 28th decimal place. This control-specific check does not change any frozen BI tolerance.
- The mixed-double control differs only in the optional e0 lexical spelling; the datatype and binary64 value agree.
- Jena's CLI throws ArithmeticException for decimal division by zero and emits incomplete JSON. That reference remains unscored. The candidate's unbound result independently satisfies its declared error contract; the oracle failure is not converted into an empty reference result.

These are retained controls evaluated against a revised implementation, not a blind holdout or general SPARQL conformance proof. Earlier unsuccessful sources and their raw evidence remain in ../exact-decimal-qualification-v1 and ../exact-decimal-qualification-v2.

## BI100M and replay evidence

The native read-only run waits for optimizer statistics, executes the fixed 60-query workload and preserves raw response bodies, hashes and frozen-policy checks. postcheck.py independently replays all three gates and checks all 60 baseline bags and sequences, source files, executable digest, reference hashes and before/after snapshot identities. postchecks.json records the results. No policy or workload was relaxed. Local startup/query observations in logs are not comparative benchmark timings.

Build logs, locked build status, source snapshot/identities, control manifests, reused independent references, replay commands, raw candidate outputs, runtime configuration, guard samples and process-exit receipts are included. Executables and Python bytecode are omitted. SHA256SUMS.json hashes every report/evidence file except itself. Historical absolute paths in scripts identify the actual run and require explicit adaptation before replay.

## Subsequent normative review: distinct from contract acceptance

The decimal-zero expected result can be established independently of Jena: [XPath 2.0 Functions and Operators (2007), numeric-divide](https://www.w3.org/TR/2007/REC-xpath-functions-20070123/#func-numeric-divide) defines decimal division by zero as an expression error, and [SPARQL 1.1 assignment](https://www.w3.org/TR/sparql11-query/#assignment) leaves the assigned variable unbound while continuing the solution. Thus this candidate's single empty binding is correct for that control. The historical Jena CLI attempt still failed and remains unscored; this is a separately sourced normative assessment, not a replacement Jena output.

The same [XPath arithmetic rules, section 6.2](https://www.w3.org/TR/2007/REC-xpath-functions-20070123/#op.numeric) require decimal underflow to return zero, and permit implementation-defined decimal rounding/truncation when precision is exceeded. Our stricter declared exact-or-error contract instead returns an error for the retained terminating underflow quotient. V3 passes the stated product contract, but this identifies a remaining standards distinction. The earlier boundary rejections tested the author's exact-or-error promise; they are not proof that every rounded result violated XPath. This caveat was sent to the implementation owner in task26 message a65841e1, and the frozen BI acceptance policies and historical classifications remain unchanged.
