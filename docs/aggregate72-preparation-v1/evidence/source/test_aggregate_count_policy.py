import copy
import unittest

from aggregate_count_policy import XSD, compare, count_value


def literal(value, datatype='integer'):
    return {'type': 'literal', 'value': str(value), 'datatype': XSD + datatype}


def result(rows, names=('key', 'c')):
    return {'head': {'vars': list(names)}, 'results': {'bindings': rows}}


class ExactCountPolicy(unittest.TestCase):
    columns = (('key', 'term'), ('c', 'count'))
    row = {'key': {'type': 'uri', 'value': 'urn:key'}, 'c': literal(17)}

    def test_canonical_and_legacy_json_literal_kind(self):
        expected = result([self.row])
        actual = copy.deepcopy(expected)
        actual['results']['bindings'][0]['c']['type'] = 'typed-literal'
        self.assertTrue(compare(actual, expected, self.columns)['primary_pass'])

    def test_derived_integer_is_compatible_but_not_exact(self):
        for datatype in ['int', 'long']:
            actual = result([{**self.row, 'c': literal(17, datatype)}])
            got = compare(actual, result([self.row]), self.columns)
            self.assertFalse(got['primary_pass'])
            self.assertTrue(got['count_compatible_pass'])

    def test_no_float_decimal_boolean_or_plain_string_count(self):
        for datatype in ['float', 'double', 'decimal', 'boolean', 'string']:
            got = compare(result([{**self.row, 'c': literal(17, datatype)}]), result([self.row]), self.columns)
            self.assertFalse(got['count_compatible_pass'])

    def test_exact_large_integer_without_float_conversion(self):
        value = 2**256 + 1
        self.assertEqual(count_value(literal(value)), value)
        self.assertNotEqual(count_value(literal(value)), int(float(value)))

    def test_declared_range_is_enforced(self):
        for bits, datatype in [(32, 'int'), (64, 'long')]:
            self.assertEqual(count_value(literal(2**(bits-1)-1, datatype)), 2**(bits-1)-1)
            with self.assertRaises(ValueError):count_value(literal(2**(bits-1), datatype))

    def test_valid_noncanonical_integer_lexical_forms(self):
        for text in ['+00017', ' \t17\r\n']:
            got = compare(result([{**self.row, 'c': literal(text, 'int')}]), result([self.row]), self.columns)
            self.assertFalse(got['primary_pass'])
            self.assertTrue(got['count_compatible_pass'])

    def test_negative_invalid_or_language_tagged_count_is_rejected(self):
        for text in ['-1', '1.0', '1e0', '1 2', 'NaN', '١٧']:
            with self.assertRaises(ValueError):count_value(literal(text))
        with self.assertRaises(ValueError):count_value({**literal(17), 'xml:lang': 'en'})

    def test_group_key_datatype_and_lexical_identity_are_preserved(self):
        a = result([{'key': literal('01', 'int'), 'c': literal(1)}])
        b = result([{'key': literal('1', 'int'), 'c': literal(1)}])
        self.assertFalse(compare(a, b, self.columns)['count_compatible_pass'])
        a = result([{'key': literal('1', 'integer'), 'c': literal(1)}])
        self.assertFalse(compare(a, b, self.columns)['count_compatible_pass'])

    def test_unbound_key_and_bag_multiplicity_are_preserved(self):
        a = result([{'c': literal(17)}])
        self.assertFalse(compare(a, result([self.row]), self.columns)['count_compatible_pass'])
        self.assertFalse(compare(result([self.row, self.row]), result([self.row]), self.columns)['count_compatible_pass'])

    def test_missing_or_extra_bindings_and_projection_are_rejected(self):
        for actual in [result([{'key': self.row['key']}]), result([{**self.row, 'extra': literal(1)}]),
                       result([self.row], ('key', 'key'))]:
            with self.assertRaises(ValueError):compare(actual, result([self.row]), self.columns)

    def test_header_order_has_no_effect_on_named_term_bag(self):
        self.assertTrue(compare(result([self.row], ('c', 'key')), result([self.row]), self.columns)['primary_pass'])

    def test_invalid_reference_cannot_authorize_a_result(self):
        bad = result([{**self.row, 'c': literal(-1)}])
        with self.assertRaises(ValueError):compare(bad, bad, self.columns)
        bad = result([{**self.row, 'c': literal(17, 'int')}])
        with self.assertRaises(ValueError):compare(bad, bad, self.columns)


if __name__ == '__main__':unittest.main()
