import json
import yaml
import sys
import argparse

# Argument parser setup
parser = argparse.ArgumentParser(description="Check for unexpected test results based on an exclusion list.")
parser.add_argument("report_json", help="Path to the hive report JSON file.")
parser.add_argument("--exclusion", required=True, help="Path to the exclusion YAML file.")
parser.add_argument("--ignored", required=True, help="Path to the ignored tests YAML file.")
args = parser.parse_args()

# Load hive JSON
with open(args.report_json, 'r') as file:
    report = json.load(file)

# Load exclusion YAML
with open(args.exclusion, 'r') as file:
    exclusion_data = yaml.safe_load(file)
    exclusions = exclusion_data.get(report['name'], [])

# Load ignored tests YAML
with open(args.ignored, 'r') as file:
    ignored_data = yaml.safe_load(file)
    ignored_tests = ignored_data.get(report['name'], [])

# Collect unexpected failures and passes
unexpected_failures = []
unexpected_passes = []
ignored_results = {'passed': [], 'failed': []}

for test in report['testCases'].values():
    test_name = test['name']
    test_pass = test['summaryResult']['pass']
    
    # Check if this is an ignored test
    if test_name in ignored_tests:
        # Track ignored test results for informational purposes
        if test_pass:
            ignored_results['passed'].append(test_name)
        else:
            ignored_results['failed'].append(test_name)
        continue  # Skip this test - don't count it as unexpected
    
    # Check against expected failures
    if test_name in exclusions:
        if test_pass:
            unexpected_passes.append(test_name)
    else:
        if not test_pass:
            unexpected_failures.append(test_name)

# Print summary of ignored tests if any were ignored
if ignored_results['passed'] or ignored_results['failed']:
    print("Ignored Tests:")
    if ignored_results['passed']:
        print(f"  Passed ({len(ignored_results['passed'])} tests):")
        for test in ignored_results['passed']:
            print(f"    {test}")
    if ignored_results['failed']:
        print(f"  Failed ({len(ignored_results['failed'])} tests):")
        for test in ignored_results['failed']:
            print(f"    {test}")
    print()

# Check if there are any unexpected failures or passes and exit with error
if unexpected_failures or unexpected_passes:
    if unexpected_failures:
        print("Unexpected Failures:")
        for test in unexpected_failures:
            print(f"  {test}")
    if unexpected_passes:
        print("Unexpected Passes:")
        for test in unexpected_passes:
            print(f"  {test}")
    sys.exit(1)

print("Success.")
