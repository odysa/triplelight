# Third-party notices

Parts of TripleLight are derived from other open-source projects. Their notices follow.

## Oxigraph (oxttl 0.2.5, oxiri 0.2.11, oxrdf 0.3.5)

`crates/triplelight-core/src/ntriples.rs` and `crates/triplelight-core/src/turtle.rs` follow the lexer and the
N-Triples, N-Quads, Turtle and TriG grammar of [oxttl](https://github.com/oxigraph/oxigraph); `crates/triplelight-iri`
follows [oxiri](https://github.com/oxigraph/oxiri)'s reference resolution where it departs from RFC 3986; and
`crates/triplelight-rdf` is a port of the RDF term types of [oxrdf](https://github.com/oxigraph/oxigraph). They were
written for task #45 to accept and produce exactly what those crates did, which are dual-licensed MIT OR Apache-2.0;
used here under the MIT license:

```
Copyright (c) 2018 Oxigraph developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

## oxilangtag 0.1.6

`crates/triplelight-rdf/src/langtag.rs` follows the RFC 5646 well-formedness rules of
[oxilangtag](https://github.com/oxigraph/oxilangtag), licensed MIT:

```
Copyright (c) 2015-2021 Pyfisch Tpt

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```
