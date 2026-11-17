"""Semantic regressions; no database or benchmark timing is simulated here."""
from dataclasses import dataclass
from types import SimpleNamespace
import unittest

from contract import XSD, check_order, json_term, python_term, signature, term


class IRI(str):
    pass


class BNode(str):
    pass


@dataclass
class Literal:
    value: str
    datatype: str | None = None
    lang: str | None = None


API = SimpleNamespace(IRI=IRI, BNode=BNode, Literal=Literal)


class Contract(unittest.TestCase):
    def test_term_identity_and_unbound(self):
        values = [Literal("1", XSD + "int"), Literal("01", XSD + "int"),
                  1, True, "", None, IRI(""), BNode("x")]
        terms = [python_term(value, API) for value in values]
        self.assertEqual(len(set(terms)), len(values))
        with self.assertRaises(TypeError):
            python_term(1.0, API)  # float erases original datatype/lexical form
        self.assertEqual(python_term(Literal("hi", lang="EN"), API),
                         json_term({"type": "literal", "value": "hi", "xml:lang": "en"}))
        self.assertEqual(json_term({"type": "literal", "value": "", "datatype": XSD + "string"}),
                         python_term("", API))

    def test_full_bag_with_projection_alignment(self):
        a, b = term("uri", "urn:a"), term("literal", "b")
        self.assertEqual(signature(["x", "y"], [(a, b), (a, b)]),
                         signature(["y", "x"], [(b, a), (b, a)]))
        self.assertNotEqual(signature(["x"], [(a,), (a,)]), signature(["x"], [(a,)]))
        self.assertNotEqual(signature(["x"], [(a,)]), signature(["y"], [(a,)]))

    def test_order_numeric_not_lexical_and_ties(self):
        spec = [{"var": "n", "kind": "numeric"}]
        rows = [(term("literal", n, XSD + "int"),) for n in ("2", "02", "10")]
        self.assertTrue(check_order(["n"], rows, spec)["checked"])
        with self.assertRaises(ValueError):
            check_order(["n"], list(reversed(rows)), spec)
        self.assertTrue(check_order(["n"], list(reversed(rows)), [{**spec[0], "descending": True}])["checked"])
        with self.assertRaises(ValueError):
            check_order(["n"], [(term("literal", "NaN", XSD + "double"),)], spec)

    def test_multikey_order_and_unbound(self):
        rows = [(None, term("literal", "z")),
                (term("literal", "a"), term("literal", "z")),
                (term("literal", "a"), term("literal", "b"))]
        spec = [{"var": "x", "kind": "lexical", "term_kind": "literal"},
                {"var": "y", "kind": "lexical", "term_kind": "literal", "descending": True}]
        self.assertTrue(check_order(["x", "y"], rows, spec)["checked"])
        with self.assertRaises(ValueError):
            check_order(["x", "y"], [rows[0], rows[2], rows[1]], spec)

    def test_bsbm_cast_and_datetime_bounds(self):
        spec = [{"var": "price", "kind": "double_cast"}]
        self.assertTrue(check_order(["price"], [(term("literal", "2"),), (term("literal", "10"),)], spec)["checked"])
        date_spec = [{"var": "date", "kind": "iso_datetime"}]
        self.assertTrue(check_order(["date"], [(term("literal", "2008-06-20T00:00:00", XSD+"dateTime"),)], date_spec)["checked"])
        with self.assertRaises(ValueError):
            check_order(["date"], [(term("literal", "2008-06-20T00:00:00Z", XSD+"dateTime"),)], date_spec)


if __name__ == "__main__":
    unittest.main()
