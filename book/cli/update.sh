#!/bin/bash

# Define the build path.
build_path=$1
if [ -z "$build_path" ]; then
  echo "Build path variable is not defined. Exiting..."
  exit 1
fi
reth_path=./$build_path/debug/reth
echo "Using reth path: $reth_path (build path: $build_path)"

# Define the path to the JSON configuration file.
json_file="./book/cli/config.json"
echo "Using config file: $json_file"

# Read commands from JSON configuration file.
read_cmds_from_json() {
  local json_file="$1"
  jq -r '.commands | keys[]' "$json_file"
}

# Read subcommands for a given command from JSON configuration file.
read_subcmds_from_json() {
  local json_file="$1"
  local cmd="$2"
  jq -r ".commands[\"$cmd\"] | if type == \"object\" then keys[] else .[] end" "$json_file"
}

# Read subsubcommands for a given command and subcommand from JSON configuration file.
read_subsubcmds_from_json() {
  local json_file="$1"
  local cmd="$2"
  local subcmd="$3"
  jq -r ".commands[\"$cmd\"][\"$subcmd\"][]" "$json_file"
}

# Update the main documentation.
update_main_doc() {
  local file_path="./book/cli/cli.md"
  local cmd_help_output=$($reth_path --help)
  sed -i -e '/## Commands/,$d' "$file_path"
  cat >> "$file_path" << EOF
## Commands

\`\`\`bash
$ reth --help
$cmd_help_output
\`\`\`
EOF
}

# Update any `reth` command documentation.
update_cli_cmd() {
  local cmd="$1"
  local subcmds=("${@:2}")
  echo "reth $cmd"

  local cmd_help_output=$($reth_path "$cmd" --help)
  local description=$(echo "$cmd_help_output" | head -n 1)
  cat > "./book/cli/$cmd.md" << EOF
# \`reth $cmd\`

$(if [[ -n "$description" ]]; then echo "$description"; fi)

\`\`\`bash
$ reth $cmd --help
$(echo "$cmd_help_output" | sed '1d')
\`\`\`
EOF

  for subcmd in "${subcmds[@]}"; do
    echo "  ├── $subcmd"

    local subcmd_help_output=$($reth_path "$cmd" "$subcmd" --help)
    local subcmd_description=$(echo "$subcmd_help_output" | head -n 1)
    cat >> "book/cli/$cmd.md" << EOF

## \`reth $cmd $subcmd\`

$(if [[ -n "$subcmd_description" ]]; then echo "$subcmd_description"; fi)

\`\`\`bash
$ reth $cmd $subcmd --help
$(echo "$subcmd_help_output" | sed '1d')
\`\`\`
EOF

    # Read subsubcommands and update documentation
    subsubcmds=($(read_subsubcmds_from_json "$json_file" "$cmd" "$subcmd"))
    for subsubcmd in "${subsubcmds[@]}"; do
      echo "    ├── $subsubcmd"

      local subsubcmd_help_output=$($reth_path "$cmd" "$subcmd" "$subsubcmd" --help)
      local subsubcmd_description=$(echo "$subsubcmd_help_output" | head -n 1)
      cat >> "book/cli/$cmd.md" << EOF

### \`reth $cmd $subcmd $subsubcmd\`

$(if [[ -n "$subsubcmd_description" ]]; then echo "$subsubcmd_description"; fi)

\`\`\`bash
$ reth $cmd $subcmd $subsubcmd --help
$(echo "$subsubcmd_help_output" | sed '1d')
\`\`\`
EOF
    done
  done
}

# Update the book CLI documentation.
main() {
  update_main_doc

  # Update commands doc.
  cmds=($(read_cmds_from_json "$json_file"))
  for cmd in "${cmds[@]}"; do
    subcmds=($(read_subcmds_from_json "$json_file" "$cmd"))
    update_cli_cmd "$cmd" "${subcmds[@]}"
  done

  # Update default paths on both Linux and macOS to avoid triggering the CI.
  sed -i -e 's/default: \/.*\/reth\//default: \/reth\//g' ./book/cli/*.md
  rm ./book/cli/*.md-e

  echo "Book updated successfully."
}

main
