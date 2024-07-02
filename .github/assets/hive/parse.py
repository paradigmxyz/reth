import json
import sys
import argparse

# Argument parser setup
parser = argparse.ArgumentParser(description="Check for unexpected test results based on an exclusion list.")
parser.add_argument("main_json", help="Path to the main JSON file.")
parser.add_argument("--exclusion", required=True, help="Path to the exclusion JSON file.")
args = parser.parse_args()

# Load main JSON
with open(args.main_json, 'r') as file:
    main_data = json.load(file)

# Load exclusion JSON
with open(args.exclusion, 'r') as file:
    exclusion_data = json.load(file)
    exclusions = exclusion_data.get(main_data['name'], [])

# Collect unexpected failures and passes
unexpected_failures = []
unexpected_passes = []

for test in main_data['testCases'].values():
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