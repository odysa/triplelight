"""Proposed exact-anchor binary32 envelopes; not wired into BI qualification.

The previous four-ULP matrices remain unchanged. Callers must independently
validate input membership/counts, datatypes and error semantics before using
these helpers. Indeterminate conditions never qualify an output.
"""
from dataclasses import dataclass
from fractions import Fraction
import re

U=Fraction(1,2**24)
DELTA=Fraction(1,2**150)
MAX_FLOAT=Fraction((2**24-1)*2**104)


class Indeterminate(ValueError):
    pass


def power2(exponent):
    return Fraction(2**exponent) if exponent>=0 else Fraction(1,2**(-exponent))


def round_binary32(value):
    """Exact nearest/ties-even conversion of a finite rational, no binary64 hop."""
    value=Fraction(value)
    if value==0:return value
    if value<0:return -round_binary32(-value)
    n,d=value.numerator,value.denominator
    exponent=n.bit_length()-d.bit_length()
    if value<power2(exponent):exponent-=1
    quantum=power2(max(exponent-23,-149))
    scaled=value/quantum
    integer,remainder=divmod(scaled.numerator,scaled.denominator)
    if 2*remainder>scaled.denominator or (2*remainder==scaled.denominator and integer%2):integer+=1
    rounded=integer*quantum
    if rounded>MAX_FLOAT:raise Indeterminate('binary32 overflow')
    return rounded


def decimal_to_binary32(lexical):
    text=lexical.strip(' \t\r\n')
    if not re.fullmatch(r'[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?',text):
        raise Indeterminate('invalid or non-finite numeric lexical form')
    try:return round_binary32(Fraction(text))
    except (ValueError,ZeroDivisionError,OverflowError) as exc:
        raise Indeterminate('invalid or non-finite numeric lexical form') from exc


@dataclass(frozen=True)
class Interval:
    lo: Fraction
    hi: Fraction

    def __post_init__(self):
        if self.lo>self.hi:raise ValueError('reversed interval')

    def contains(self,value):
        return self.lo<=Fraction(value)<=self.hi

    def record(self):
        return {'lo':str(self.lo),'hi':str(self.hi)}


@dataclass
class InputStats:
    n: int=0
    total: Fraction=Fraction(0)
    magnitude: Fraction=Fraction(0)

    def add(self,value):
        value=Fraction(value)
        if round_binary32(value)!=value:raise ValueError('input must already be a binary32 cast value')
        self.n+=1;self.total+=value;self.magnitude+=abs(value)

    @classmethod
    def from_values(cls,values):
        stats=cls()
        for value in values:stats.add(value)
        return stats

    def record(self):
        return {'n':self.n,'exact_sum':str(self.total),'exact_sum_magnitudes':str(self.magnitude)}


def sum_envelope(stats):
    if type(stats.n) is not int or stats.n<0 or stats.magnitude<abs(stats.total):
        raise ValueError('invalid input statistics')
    if stats.n==0:
        if stats.total or stats.magnitude:raise ValueError('nonzero empty input statistics')
        return Interval(Fraction(0),Fraction(0))
    nu=stats.n*U
    if nu>=1:raise Indeterminate('n*u is not below one')
    error=nu/(1-nu)*stats.magnitude+stats.n*DELTA/(1-nu)
    # Bounds every partial sum by input magnitudes, regardless of cancellation.
    if stats.magnitude+error>MAX_FLOAT:raise Indeterminate('cannot exclude intermediate overflow')
    return Interval(stats.total-error,stats.total+error)


def mean_envelope(stats):
    if stats.n==0:raise Indeterminate('empty AVG requires its separate typed SPARQL rule')
    if stats.n>2**24:raise Indeterminate('integer count is not guaranteed exact in binary32')
    summed=sum_envelope(stats)
    rounding=U*max(abs(summed.lo),abs(summed.hi))/stats.n+DELTA
    return Interval(summed.lo/stats.n-rounding,summed.hi/stats.n+rounding)


def divide_envelope(numerator,denominator):
    if denominator.lo<=0<=denominator.hi:raise Indeterminate('denominator interval includes zero')
    quotients=[a/b for a in (numerator.lo,numerator.hi) for b in (denominator.lo,denominator.hi)]
    low,high=min(quotients),max(quotients)
    rounding=U*max(abs(low),abs(high))+DELTA
    if max(abs(low),abs(high))+rounding>MAX_FLOAT:raise Indeterminate('cannot exclude division overflow')
    return Interval(low-rounding,high+rounding)
