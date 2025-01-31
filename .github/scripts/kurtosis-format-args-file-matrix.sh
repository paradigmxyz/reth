#!/usr/bin/env bash

# 
# 
# This is a log script used mostly in the CI pipeline
# 
# It will print a JSON formatted array of kurtosis args files
# passed in as an input to this script
# 
# Usage:
# 
# ./kurtosis-format-args-file-matrix.sh <ARG_FILE> [...<ARG_FILE>]
# 
#

# This scripts expects an array of args files as its arguments
ARGS_FILES=("$@")

# All the echoes must go to stderr since stdout must be a JSON object
echo "Found ${#ARGS_FILES[@]} matching files: ${ARGS_FILES[@]}" 1>&2;

ARGS=()
for ARGS_FILE_PATH in "${ARGS_FILES[@]}"; do
    # We get the kurtosis file basename
    ARGS_FILE_NAME=$(basename "$ARGS_FILE_PATH")
    # And try to remove the prefix to get the important version of the filename
    ARGS_FILE_SUBNAME=$(echo "$ARGS_FILE_NAME" | awk 'sub(/kurtosis_op_network_params_/,"")')
    
    # Now we turn those into an object
    ARGS_OBJECT=$(jq -cn --arg path "$ARGS_FILE_PATH" --arg name "$ARGS_FILE_NAME" --arg subname "$ARGS_FILE_SUBNAME" '{path: $path, name: $name, subname: $subname}')

    # And push it to the ARGS array
    ARGS+=("$ARGS_OBJECT")
done

# We combine all the individual objects into a JSON array
ARGS_JSON=$(printf '%s\n' "${ARGS[@]}" | jq -sc .)

# And put the array under an "args" key in an object
# 
# This way the output can be directly used as a github action matrix definition
ARGS_FILES_MATRIX=$(jq -cn --argjson array "$ARGS_JSON" '{args: $array}')

# Print the matrix to stdout
echo "$ARGS_FILES_MATRIX"