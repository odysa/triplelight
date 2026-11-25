"""BSBM BI-10M entry point for the frozen BI case policy."""

import bi_case_policy


NAME = "bsbm-bi-frozen-v1"
DATASET_SHA256 = "3a207c6725820036eec87d08dd414090d3548ac2839ca192414dad6e352f680e"


def start(context):
    bi_case_policy.start(context, DATASET_SHA256)


def check(case_id, observed_answer, reference_record):
    return bi_case_policy.check(case_id, observed_answer, reference_record)
