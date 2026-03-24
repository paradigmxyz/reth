#!/usr/bin/env -S cargo +nightly -Zscript
---
[package]
edition = "2021"

[dependencies]
clap = { version = "4", features = ["derive"] }
regex = "1"
---
use clap::Parser;
use regex::Regex;
use std::{
    borrow::Cow,
    collections::HashSet,
    fmt, fs, io,
    iter::once,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str,
    sync::LazyLock,
};

const README: &str = r#"import Summary from './SUMMARY.mdx';

# CLI Reference

Automatically-generated CLI reference from `--help` output.

<Summary />
"#;
const TRIM_LINE_END_MARKDOWN: bool = true;

/// Lazy static regex to avoid recompiling the same regex pattern multiple times.
macro_rules! regex {
    ($re:expr) => {{
        static RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new($re).expect("Failed to compile regex pattern"));
        &*RE
    }};
}

/// Generate markdown files from the help output of commands
#[derive(Parser, Debug)]
#[command(about, long_about = None)]
struct Args {
    /// Root directory
    #[arg(long, default_value_t = String::from("."))]
    root_dir: String,

    /// Indentation for the root SUMMARY.mdx file
    #[arg(long, default_value_t = 2)]
    root_indentation: usize,

    /// Output directory
    #[arg(long)]
    out_dir: PathBuf,

    /// Whether to add a README.md file
    #[arg(long)]
    readme: bool,

    /// Whether to update the root SUMMARY.mdx file
    #[arg(long)]
    root_summary: bool,

    /// Whether to generate TypeScript sidebar files
    #[arg(long)]
    sidebar: bool,

    /// Print verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Commands to generate markdown for.
    #[arg(required = true, num_args = 1..)]
    commands: Vec<PathBuf>,
}

fn write_file(file_path: &Path, content: &str) -> io::Result<()> {
    let content = if TRIM_LINE_END_MARKDOWN {
        content.lines().map(|line| line.trim_end()).collect::<Vec<_>>().join("\n")
    } else {
        content.to_string()
    };
    fs::write(file_path, content)
}

/// Recursively collects all `.mdx` files in a directory.
fn collect_mdx_files(dir: &Path) -> HashSet<PathBuf> {
    let mut files = HashSet::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_mdx_files(&path));
            } else if path.extension().is_some_and(|ext| ext == "mdx") {
                files.insert(path);
            }
        }
    }
    files
}

/// Recursively removes empty directories under `dir`.
fn remove_empty_dirs(dir: &Path) -> io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            remove_empty_dirs(&path)?;
            // Remove the directory if it's now empty.
            if fs::read_dir(&path)?.next().is_none() {
                println!("  Removing empty directory: {}", path.display());
                fs::remove_dir(&path)?;
            }
        }
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    debug_assert!(args.commands.len() >= 1);

    let out_dir = args.out_dir;
    fs::create_dir_all(&out_dir)?;

    let mut todo_iter: Vec<Cmd> = args
        .commands
        .iter()
        .rev() // reverse to keep the order (pop)
        .map(Cmd::new)
        .collect();
    let mut output = Vec::new();

    // Iterate over all commands and their subcommands.
    while let Some(cmd) = todo_iter.pop() {
        let (new_subcmds, stdout) = get_entry(&cmd)?;
        if args.verbose && !new_subcmds.is_empty() {
            println!("Found subcommands for \"{}\": {:?}", cmd.command_name(), new_subcmds);
        }
        // Add new subcommands to todo_iter (so that they are processed in the correct order).
        for subcmd in new_subcmds.into_iter().rev() {
            let new_subcmds: Vec<_> = cmd.subcommands.iter().cloned().chain(once(subcmd)).collect();

            todo_iter.push(Cmd { cmd: cmd.cmd, subcommands: new_subcmds });
        }
        output.push((cmd, stdout));
    }

    // Collect existing .mdx files that were previously generated so we can detect stale ones.
    // Only consider files within command subdirectories and known generated top-level files.
    let mut existing_files = HashSet::new();
    for (cmd, _) in &output {
        let cmd_name = cmd.command_name();
        // Collect the top-level command file (e.g. reth.mdx)
        let top_level = out_dir.join(cmd_name).with_extension("mdx");
        if top_level.exists() {
            existing_files.insert(top_level);
        }
        // Collect all .mdx files in the command's subdirectory (e.g. reth/)
        existing_files.extend(collect_mdx_files(&out_dir.join(cmd_name)));
    }
    // Also track SUMMARY.mdx
    let summary_file = out_dir.join("SUMMARY.mdx");
    if summary_file.exists() {
        existing_files.insert(summary_file);
    }
    let mut written_files: HashSet<PathBuf> = HashSet::new();

    // Generate markdown files.
    for (cmd, stdout) in &output {
        let path = cmd_markdown(&out_dir, cmd, stdout)?;
        written_files.insert(path);
    }

    // Generate SUMMARY.mdx.
    let summary: String =
        output.iter().map(|(cmd, _)| cmd_summary(cmd, 0)).chain(once("\n".to_string())).collect();

    let summary_path = out_dir.join("SUMMARY.mdx");
    println!("Writing SUMMARY.mdx to \"{}\"", out_dir.to_string_lossy());
    write_file(&summary_path, &summary)?;
    written_files.insert(summary_path);

    // Generate README.md.
    if args.readme {
        let path = out_dir.join("README.mdx");
        if args.verbose {
            println!("Writing README.mdx to \"{}\"", path.display());
        }
        write_file(&path, README)?;
        written_files.insert(path);
    }

    // Generate root SUMMARY.mdx.
    if args.root_summary {
        let root_summary: String = output
            .iter()
            .map(|(cmd, _)| cmd_summary(cmd, args.root_indentation))
            .chain(once("\n".to_string()))
            .collect();

        let path = Path::new(args.root_dir.as_str());
        if args.verbose {
            println!("Updating root summary in \"{}\"", path.to_string_lossy());
        }
        update_root_summary(path, &root_summary)?;
    }

    // Generate TypeScript sidebar files.
    if args.sidebar {
        let vocs_dir = Path::new(args.root_dir.as_str()).join("vocs");
        if args.verbose {
            println!("Generating TypeScript sidebar files in \"{}\"", vocs_dir.display());
        }
        generate_sidebar_files(&vocs_dir, &output, args.verbose)?;
    }

    // Remove stale .mdx files that were not regenerated.
    let stale_files: Vec<_> = existing_files.difference(&written_files).collect();
    if !stale_files.is_empty() {
        println!("Removing {} stale file(s):", stale_files.len());
        for file in &stale_files {
            println!("  - {}", file.display());
            fs::remove_file(file)?;
        }
        remove_empty_dirs(&out_dir)?;
    }

    Ok(())
}

/// Returns the subcommands and help output for a command.
fn get_entry(cmd: &Cmd) -> io::Result<(Vec<String>, String)> {
    let output = Command::new(cmd.cmd)
        .args(&cmd.subcommands)
        .arg("--help")
        .env("NO_COLOR", "1")
        .env("COLUMNS", "100")
        .env("LINES", "10000")
        .stdout(Stdio::piped())
        .output()?;

    if !output.status.success() {
        let stderr = str::from_utf8(&output.stderr).unwrap_or("Failed to parse stderr as UTF-8");
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Command \"{}\" failed:\n{}", cmd, stderr),
        ));
    }

    let stdout = str::from_utf8(&output.stdout)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
        .to_string();

    // Parse subcommands from the help output
    let subcmds = parse_sub_commands(&stdout);

    Ok((subcmds, stdout))
}

/// Returns a list of subcommands from the help output of a command.
fn parse_sub_commands(s: &str) -> Vec<String> {
    // This regex matches lines starting with two spaces, followed by the subcommand name.
    let re = regex!(r"^  (\S+)");

    s.split("Commands:")
        .nth(1) // Get the part after "Commands:"
        .map(|commands_section| {
            commands_section
                .lines()
                .take_while(|line| !line.starts_with("Options:") && !line.starts_with("Arguments:"))
                .filter_map(|line| {
                    re.captures(line).and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
                })
                .filter(|cmd| cmd != "help")
                .map(String::from)
                .collect()
        })
        .unwrap_or_default() // Return an empty Vec if "Commands:" was not found
}

/// Writes the markdown for a command to out_dir. Returns the path of the written file.
fn cmd_markdown(out_dir: &Path, cmd: &Cmd, stdout: &str) -> io::Result<PathBuf> {
    let out = format!("# {}\n\n{}", cmd, help_markdown(cmd, stdout));

    let out_path = out_dir.join(cmd.to_string().replace(" ", "/"));
    fs::create_dir_all(out_path.parent().unwrap())?;
    let file_path = out_path.with_extension("mdx");
    write_file(&file_path, &out)?;

    Ok(file_path)
}

/// Returns the markdown for a command's help output.
fn help_markdown(cmd: &Cmd, stdout: &str) -> String {
    let (description, s) = parse_description(stdout);
    format!(
        "{}\n\n```bash\n$ {} --help\n```\n```txt\n{}\n```",
        description,
        cmd,
        preprocess_help(s.trim())
    )
}

/// Splits the help output into a description and the rest.
fn parse_description(s: &str) -> (&str, &str) {
    match s.find("Usage:") {
        Some(idx) => {
            let description = s[..idx].trim().lines().next().unwrap_or("");
            (description, &s[idx..])
        }
        None => ("", s),
    }
}

/// Returns the summary for a command and its subcommands.
fn cmd_summary(cmd: &Cmd, indent: usize) -> String {
    let cmd_s = cmd.to_string();
    let cmd_path = cmd_s.replace(" ", "/");
    let indent_string = " ".repeat(indent + (cmd.subcommands.len() * 2));
    format!("{}- [`{}`](./{}.mdx)\n", indent_string, cmd_s, cmd_path)
}

/// Overwrites the root SUMMARY.mdx file with the generated content.
fn update_root_summary(root_dir: &Path, root_summary: &str) -> io::Result<()> {
    let summary_file = root_dir.join("vocs/docs/pages/cli/SUMMARY.mdx");
    println!("Overwriting {}", summary_file.display());

    // Simply write the root summary content to the file
    write_file(&summary_file, root_summary)
}

/// Generates TypeScript sidebar files for each command.
fn generate_sidebar_files(
    vocs_dir: &Path,
    output: &[(Cmd, String)],
    verbose: bool,
) -> io::Result<()> {
    // Group commands by their root command name
    // Also create a map of commands to their help output
    let mut commands_by_root: std::collections::HashMap<String, Vec<&Cmd>> =
        std::collections::HashMap::new();
    let mut help_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    for (cmd, help_output) in output {
        let root_name = cmd.command_name().to_string();
        commands_by_root.entry(root_name.clone()).or_insert_with(Vec::new).push(cmd);
        // Store help output for each command using its string representation as key
        help_map.insert(cmd.to_string(), help_output.clone());
    }

    // Generate sidebar file for each root command
    for (root_name, cmds) in commands_by_root {
        // Get root help output
        let root_help = help_map.get(&root_name).map(|s| s.as_str());
        let sidebar_content = generate_sidebar_ts(&root_name, cmds, root_help, &help_map)?;
        let file_name = match root_name.as_str() {
            "reth" => "sidebar-cli-reth.ts",
            _ => {
                if verbose {
                    println!("Skipping unknown command: {}", root_name);
                }
                continue;
            }
        };

        let sidebar_file = vocs_dir.join(file_name);
        if verbose {
            println!("Writing sidebar file: {}", sidebar_file.display());
        }
        write_file(&sidebar_file, &sidebar_content)?;
    }

    Ok(())
}

/// Generates TypeScript code for a sidebar file.
fn generate_sidebar_ts(
    root_name: &str,
    commands: Vec<&Cmd>,
    root_help: Option<&str>,
    help_map: &std::collections::HashMap<String, String>,
) -> io::Result<String> {
    // Find all top-level commands (commands with exactly one subcommand)
    let mut top_level_commands: Vec<&Cmd> =
        commands.iter().copied().filter(|cmd| cmd.subcommands.len() == 1).collect();

    // Remove duplicates using a set
    let mut seen = std::collections::HashSet::new();
    top_level_commands.retain(|cmd| {
        let key = &cmd.subcommands[0];
        seen.insert(key.clone())
    });

    // Sort by the order they appear in help output, not alphabetically
    if let Some(help) = root_help {
        let help_order = parse_sub_commands(help);
        top_level_commands.sort_by(|a, b| {
            let a_name = &a.subcommands[0];
            let b_name = &b.subcommands[0];
            let a_pos = help_order.iter().position(|x| x == a_name).unwrap_or(usize::MAX);
            let b_pos = help_order.iter().position(|x| x == b_name).unwrap_or(usize::MAX);
            a_pos.cmp(&b_pos)
        });
    }

    // Generate TypeScript code
    let var_name = match root_name {
        "reth" => "rethCliSidebar",
        _ => "cliSidebar",
    };

    let mut ts_code = String::from("import { SidebarItem } from \"vocs\";\n\n");
    ts_code.push_str(&format!("export const {}: SidebarItem = {{\n", var_name));
    ts_code.push_str(&format!("    text: \"{}\",\n", root_name));
    ts_code.push_str(&format!("    link: \"/cli/{}\",\n", root_name));
    ts_code.push_str("    collapsed: false,\n");
    ts_code.push_str("    items: [\n");

    for (idx, cmd) in top_level_commands.iter().enumerate() {
        let is_last = idx == top_level_commands.len() - 1;
        if let Some(item_str) = build_sidebar_item(root_name, cmd, &commands, 1, help_map, is_last)
        {
            ts_code.push_str(&item_str);
        }
    }

    ts_code.push_str("    ]\n");
    ts_code.push_str("};\n\n");

    Ok(ts_code)
}

/// Builds a sidebar item for a command and its children.
/// Returns TypeScript code string.
fn build_sidebar_item(
    root_name: &str,
    cmd: &Cmd,
    all_commands: &[&Cmd],
    depth: usize,
    help_map: &std::collections::HashMap<String, String>,
    is_last: bool,
) -> Option<String> {
    let full_cmd_name = cmd.to_string();
    let link_path = format!("/cli/{}", full_cmd_name.replace(" ", "/"));

    // Find all direct child commands (commands whose subcommands start with this command's
    // subcommands)
    let mut children: Vec<&Cmd> = all_commands
        .iter()
        .copied()
        .filter(|other_cmd| {
            other_cmd.subcommands.len() == cmd.subcommands.len() + 1 &&
                other_cmd.subcommands[..cmd.subcommands.len()] == cmd.subcommands[..]
        })
        .collect();

    // Sort children by the order they appear in help output, not alphabetically
    if children.len() > 1 {
        // Get help output for this command to determine subcommand order
        if let Some(help_output) = help_map.get(&full_cmd_name) {
            let help_order = parse_sub_commands(help_output);
            children.sort_by(|a, b| {
                let a_name = a.subcommands.last().unwrap();
                let b_name = b.subcommands.last().unwrap();
                let a_pos = help_order.iter().position(|x| x == a_name).unwrap_or(usize::MAX);
                let b_pos = help_order.iter().position(|x| x == b_name).unwrap_or(usize::MAX);
                a_pos.cmp(&b_pos)
            });
        } else {
            // Fall back to alphabetical if we can't get help
            children
                .sort_by(|a, b| a.subcommands.last().unwrap().cmp(b.subcommands.last().unwrap()));
        }
    }

    let indent = "        ".repeat(depth);
    let mut item_str = String::new();

    item_str.push_str(&format!("{}{{\n", indent));
    item_str.push_str(&format!("{}    text: \"{}\",\n", indent, full_cmd_name));
    item_str.push_str(&format!("{}    link: \"{}\"", indent, link_path));

    if !children.is_empty() {
        item_str.push_str(",\n");
        item_str.push_str(&format!("{}    collapsed: true,\n", indent));
        item_str.push_str(&format!("{}    items: [\n", indent));

        for (idx, child_cmd) in children.iter().enumerate() {
            let child_is_last = idx == children.len() - 1;
            if let Some(child_str) = build_sidebar_item(
                root_name,
                child_cmd,
                all_commands,
                depth + 1,
                help_map,
                child_is_last,
            ) {
                item_str.push_str(&child_str);
            }
        }

        item_str.push_str(&format!("{}    ]\n", indent));
        if is_last {
            item_str.push_str(&format!("{}}}\n", indent));
        } else {
            item_str.push_str(&format!("{}}},\n", indent));
        }
    } else {
        item_str.push_str("\n");
        if is_last {
            item_str.push_str(&format!("{}}}\n", indent));
        } else {
            item_str.push_str(&format!("{}}},\n", indent));
        }
    }

    Some(item_str)
}

/// Preprocesses the help output of a command.
fn preprocess_help(s: &str) -> Cow<'_, str> {
    static REPLACEMENTS: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
        let patterns: &[(&str, &str)] = &[
            // Remove the user-specific paths.
            (r"default: /.*/reth", "default: <CACHE_DIR>"),
            // Remove the commit SHA and target architecture triple or fourth
            //  rustup available targets:
            //    aarch64-apple-darwin
            //    x86_64-unknown-linux-gnu
            (
                r"default: reth/.*-[0-9A-Fa-f]{6,10}/([_\w]+)-(\w+)-(\w+)(-\w+)?",
                "default: reth/<VERSION>-<SHA>/<ARCH>",
            ),
            // Remove the OS
            (r"default: reth/.*/\w+", "default: reth/<VERSION>/<OS>"),
            // Remove rpc.max-tracing-requests default value
            (
                r"(rpc.max-tracing-requests <COUNT>\n.*\n.*\n.*\n.*\n.*)\[default: \d+\]",
                r"$1[default: <NUM CPU CORES-2>]",
            ),
            // Handle engine.reserved-cpu-cores dynamic default
            (
                r"(engine\.reserved-cpu-cores.*)\[default: \d+\]",
                r"$1[default: <DYNAMIC: min(2, CPU cores)>]",
            ),
        ];
        patterns
            .iter()
            .map(|&(re, replace_with)| (Regex::new(re).expect(re), replace_with))
            .collect()
    });

    let mut s = Cow::Borrowed(s);
    for (re, replacement) in REPLACEMENTS.iter() {
        if let Cow::Owned(result) = re.replace_all(&s, *replacement) {
            s = Cow::Owned(result);
        }
    }
    s
}

#[derive(Hash, Debug, PartialEq, Eq)]
struct Cmd<'a> {
    /// path to binary (e.g. ./target/debug/reth)
    cmd: &'a Path,
    /// subcommands (e.g. [db, stats])
    subcommands: Vec<String>,
}

impl<'a> Cmd<'a> {
    fn command_name(&self) -> &str {
        self.cmd.file_name().and_then(|os_str| os_str.to_str()).expect("Expect valid command")
    }

    fn new(cmd: &'a PathBuf) -> Self {
        Self { cmd, subcommands: Vec::new() }
    }
}

impl<'a> fmt::Display for Cmd<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.command_name())?;
        if !self.subcommands.is_empty() {
            write!(f, " {}", self.subcommands.join(" "))?;
        }
        Ok(())
    }
}
