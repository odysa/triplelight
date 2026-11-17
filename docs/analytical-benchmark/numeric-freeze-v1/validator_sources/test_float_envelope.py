from fractions import Fraction as F
from itertools import permutations
import unittest

from float_envelope import (InputStats,Interval,Indeterminate,MAX_FLOAT,
                            round_binary32,decimal_to_binary32,mean_envelope,
                            sum_envelope,divide_envelope)


def sequential(values):
    total=F(0)
    for value in values:total=round_binary32(total+value)
    return total


class ProposedEnvelopes(unittest.TestCase):
    def test_exact_cast_rounds_ties_to_even(self):
        self.assertEqual(decimal_to_binary32('16777217'),F(16777216))
        self.assertEqual(decimal_to_binary32('16777219'),F(16777220))
        self.assertEqual(round_binary32(F(1,2**150)),0)
        self.assertEqual(round_binary32(F(3,2**150)),F(2,2**149))
        self.assertEqual(decimal_to_binary32('-0.5'),F(-1,2))

    def test_invalid_and_nonfinite_inputs_are_not_numbers(self):
        for text in ['bad','NaN','INF','-INF','1/2','1_0']:
            with self.subTest(text=text),self.assertRaises(Indeterminate):decimal_to_binary32(text)

    def test_long_sum_and_mean_contain_explicit_rounded_arithmetic(self):
        values=[decimal_to_binary32('0.1')]*10000
        stats=InputStats.from_values(values);computed=sequential(values)
        self.assertTrue(sum_envelope(stats).contains(computed))
        self.assertTrue(mean_envelope(stats).contains(round_binary32(computed/stats.n)))
        self.assertFalse(mean_envelope(stats).contains(F(1)))

    def test_cancellation_is_bounded_in_absolute_units(self):
        values=[F(2**24),F(1),F(-2**24)]
        envelope=sum_envelope(InputStats.from_values(values))
        witnesses={sequential(order) for order in permutations(values)}
        self.assertGreater(len(witnesses),1)
        self.assertTrue(all(envelope.contains(value) for value in witnesses))
        zero=InputStats.from_values([F(2**24),F(-2**24)])
        self.assertTrue(sum_envelope(zero).contains(0))

    def test_subnormal_mean_and_ratio(self):
        tiny=F(1,2**149);stats=InputStats.from_values([tiny,2*tiny])
        self.assertTrue(mean_envelope(stats).contains(round_binary32(F(3,2)*tiny)))
        numerator=mean_envelope(InputStats.from_values([F(2),F(4)]))
        denominator=mean_envelope(InputStats.from_values([F(1),F(2)]))
        ratio=divide_envelope(numerator,denominator)
        self.assertTrue(ratio.contains(2));self.assertFalse(ratio.contains(3))

    def test_indeterminate_cases_never_qualify(self):
        for stats in [InputStats.from_values([]),InputStats(2**24+1,F(0),F(0))]:
            with self.assertRaises(Indeterminate):mean_envelope(stats)
        with self.assertRaises(Indeterminate):sum_envelope(InputStats.from_values([MAX_FLOAT,MAX_FLOAT]))
        with self.assertRaises(Indeterminate):divide_envelope(Interval(F(1),F(2)),Interval(F(-1),F(1)))
        with self.assertRaises(Indeterminate):divide_envelope(Interval(MAX_FLOAT,MAX_FLOAT),Interval(F(1,2),F(1,2)))

    def test_unrounded_input_and_invalid_statistics_rejected(self):
        with self.assertRaises(ValueError):InputStats.from_values([F(1,10)])
        with self.assertRaises(ValueError):sum_envelope(InputStats(-1,F(0),F(0)))
        with self.assertRaises(ValueError):sum_envelope(InputStats(1,F(2),F(1)))
        with self.assertRaises(ValueError):sum_envelope(InputStats(0,F(1),F(1)))


if __name__=='__main__':unittest.main()
