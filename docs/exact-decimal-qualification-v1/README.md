# Exact decimal candidate: qualification blocked

The clean source at `09e10041ceb56abe926fecfe53be2b65272c1198` builds with `cargo build --locked --release --bin rdfdb -j1`, but independent decimal controls expose a ROUND panic, silent loss of a nonzero decimal below scale 28, and incorrect decimal-to-float promotion. No BI100M query run was started after these findings. The earlier performance tables retain their original source and results.

## Actionable controls

| Control | Candidate | Jena 6.2.0 |
| --- | --- | --- |
| ROUND(79228162514264337593543950335.0) | Process exits 101: Addition overflowed | Same integer-valued decimal |
| 0.00000000000000000000000000001 + 0.0 | Decimal 0.0 | Preserves the nonzero value |
| Decimal 16777217 + float -16777216 | Float 1.0 | Float 0.0 |
| Decimal 16777217 = float 16777216 | false | true |

The ROUND implementation adds 0.5 before floor using an overflowing Decimal addition. Float arithmetic/comparison convert operands to f64, then round only the result; promotion to float must first round the decimal operand. These code explanations support the reduced controls, not a claim about all numeric behavior.

The second control script records 25 requests: 13 exact-term matches, 11 exact-term differences and one unavailable reference. These are control requests, not a blind or deduplicated workload. The differences are classified separately: coefficient/integer overflow reflects bounded ranges; repeating division has different precision; the mixed-double result differs only in lexical exponent spelling; two cases exercise the same sub-scale value. They must not all be counted as semantic bugs.

Jena's decimal-division-by-zero CLI execution throws ArithmeticException and leaves incomplete JSON. That case is unscored against Jena; candidate returns an unbound projection. The first control attempt stopped during oracle collection on that exception and never entered the candidate loop. A separate second attempt preserves the reference error and runs the remaining controls. All raw attempts remain visible.

The earlier pin `186da6069cbcb36ace254ec9653fc89dbc36bbaf` failed its clean locked build because Cargo.toml declares rust_decimal while Cargo.lock has no corresponding package entry. Its process was observed absent, but an exact exit code was not captured. The fixed pin changes only Cargo.lock. Neither source was silently repaired by this qualification.

Evidence includes exact source manifests, build commands/logs, fixed-pin exit code 0, executable hash, control queries/commands, raw outputs and stderr. The executable itself and bytecode are omitted. Source and binary identities were verified after controls. Each query subprocess was awaited, including failures, before the exact owned benchmark lock was released. No general W3C conformance or arbitrary-precision exactness is established here. SHA256SUMS.json hashes every report/evidence file except itself.
