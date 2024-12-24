import json
import yaml
import sys
import argparse

# Argument parser setup
parser = argparse.ArgumentParser(description="Check for unexpected test results based on an exclusion list.")
parser.add_argument("report_json", help="Path to the hive report JSON file.")
parser.add_argument("--exclusion", required=True, help="Path to the exclusion YAML file.")
args = parser.parse_args()

# Load hive JSON
with open(args.report_json, 'r') as file:
    report = json.load(file)

# Load exclusion YAML
with open(args.exclusion, 'r') as file:
    exclusion_data = yaml.safe_load(file)
    exclusions = exclusion_data.get(report['name'], [])

# Collect unexpected failures and passes
unexpected_failures = []
unexpected_passes = []

for test in report['testCases'].values():
    test_name = test['name']
    test_pass = test['summaryResult']['pass']
    if test_name in exclusions:
        if test_pass:
            unexpected_passes.append(test_name)
    else:
        if not test_pass:
            unexpected_failures.append(test_name)

# Check if there are any unexpected failures or passes and exit with error
if unexpected_failures or unexpected_passes:
    if unexpected_failures:
        print("Unexpected Failures:", unexpected_failures)
    if unexpected_passes:
        print("Unexpected Passes:", unexpected_passes)
    sys.exit(1)

print("Success.")