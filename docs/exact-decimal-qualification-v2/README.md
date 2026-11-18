# Decimal v2 boundary qualification: still blocked

Candidate `af856dc0566822ebf3d6f1ffc505789731393ed0` builds cleanly with a locked release build. It fixes the previous ROUND panic and decimal-to-float promotion controls, and changes the previously silently rounded scale-29 literal into an error, as its declared contract requires. New boundary controls still return rounded values where that contract promises an exact result or an error. No BI100M queries were run after these findings.

## Contract and independent controls

The author explicitly bounds decimal representation to a 96-bit mantissa and at most 28 fractional digits, with unrepresentable literals and exact addition/subtraction/multiplication results treated as SPARQL errors. Non-terminating division may round with implementation-defined precision. This qualification tests that stated contract; it does not claim arbitrary-precision decimal support.

| Query | Candidate | Independent Jena 6.2 result / exact value |
| --- | --- | --- |
| 79228162514264337593543950335.0 + 0.1 | 79228162514264337593543950335.0 | 79228162514264337593543950335.1 |
| 0.1 + 79228162514264337593543950335.0 | Unbound | 79228162514264337593543950335.1 |
| 99999999999999.9 * 99999999999999.9 | 9999999999999980000000000000.0 | 9999999999999980000000000000.01 |
| 0.0000000000000000000000000001 / 10.0 | 0.0 | 0.00000000000000000000000000001 |

The first and third results are not representable exactly by this decimal type. Returning an error would satisfy the stated bounded contract; silently dropping the fraction does not. The reversed addition already returns an error, demonstrating operand-order dependence in the guard. The reverse subtraction used to check addition exactness also loses precision, so equality after reversing is insufficient proof. Multiplication checks only the sum of scales; reducing scale to fit an overflowing mantissa still loses digits. The division case has a terminating, nonzero exact quotient, distinct from the permitted rounding of non-terminating quotients.

The driver retains 29 requests, including the prior controls and four new boundaries, registered before this candidate's outputs were collected. It records 16 exact-term matches, 12 exact-term differences and one Jena oracle failure (decimal division by zero throws ArithmeticException in the CLI). These are request-level comparisons, not 12 semantic bugs or 29 distinct query texts: range-limit differences, repeating-division precision differences and double lexical spelling are reported separately in raw outputs. The duplicate scale-29 controls are retained. Jena's failed case is unscored, not treated as an empty answer.

All raw queries, reference/candidate outputs, stderr, commands, build receipt and source identities are retained. Source and binary hashes are unchanged at postcheck. The build and control processes exited before the exact owned lock was released. No executable is committed; its digest remains. This is correctness evidence, without comparative performance or global conformance claims. Prior failures remain in ../exact-decimal-qualification-v1.

SHA256SUMS.json hashes every report/evidence file except itself. Absolute paths in replay scripts are historical provenance and require explicit adaptation before replay.
