# Numbers without Terms for BIND / SELECT arithmetic and HAVING (task #121)

BI q3 computes `xsd:float(?monthCount) / ?monthBeforeCount` for every joined product (153k rows on m002) and filters
its second subquery with `HAVING (count(?review) > 0)`. Both went through the general evaluator: each row decoded its
inputs to `Term`s, built the result `Term`, and interned it through the query's scratch dictionary.

Now `NumExpr` (the compiled numeric filter expression) also covers `/` and `xsd:float(x)`, and is used for Extend
and for the general FILTER path (HAVING): each distinct input ID is decoded once, the value is computed on `Num`, and
only distinct results become terms (`NumKey` memo). A row the numeric path cannot evaluate (a non-numeric input)
takes the general evaluator, so values are those of `value`/`ebv` by construction. `xsd:double` casts are left on the
general path: their lexical form differs from `Num::Dbl`'s.

`numeric_extend_and_having_match_the_general_evaluator` compares seven expressions and four HAVING conditions with the
same expressions wrapped in `COALESCE` (which the numeric path does not compile) over integers, decimals, floats,
doubles, zero divisors, overflow, non-numeric literals and unbound values.

CLI ABBA on BSBM-100M (main `a10bf73d` against the branch, blocks main, branch, branch, main, 1 untimed + 3 timed per
block, medians), stdout SHA-256 identical across all runs and both arms:

| Query | main (ms) | branch (ms) | ratio |
| --- | ---: | ---: | ---: |
| q3 m000 | 301.4 | 230.9 | 0.77 |
| q3 m001 | 764.7 | 562.5 | 0.74 |
| q3 m002 | 1037.1 | 750.4 | 0.72 |
| q3 m003 | 903.6 | 670.1 | 0.74 |
| q2 (four) | 108–143 | 108–137 | 0.95–1.00 |

`ab.py` reproduces it (`BSBM100_STORE`, `BI_QUERIES`, `TL_MAIN`, `TL_CAND`). Laptop mechanism evidence only.
