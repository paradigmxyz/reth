import os
import argparse

def merge_solidity_files(root_dir, output_file='../out/taiko_protocol.md'):
    with open(output_file, 'w') as outfile:
        for subdir, dirs, files in os.walk(root_dir):
            for file in files:
                if file.endswith('.sol') and not file.endswith('.t.sol'):
                    file_path = os.path.join(subdir, file)
                    if "/test/" in file_path:
                        continue
                    print("merging ", file_path)
                    relative_path = os.path.relpath(file_path, root_dir)
                    outfile.write(f"## {relative_path}\n")
                    outfile.write("```solidity\n")
                    with open(file_path, 'r') as infile:
                        outfile.write(infile.read())
                    outfile.write("\n```\n\n")


if __name__ == "__main__":
    # parser = argparse.ArgumentParser(description="Merge Solidity files into a Markdown file.")
    # parser.add_argument("root_dir", type=str, help="Root directory containing Solidity files")
    # args = parser.parse_args()
    # merge_solidity_files(args.root_dir)
    merge_solidity_files("../contracts")
    print("merged into ../out/taiko_protocol.md")
