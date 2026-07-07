// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut files: Vec<String> = Vec::new();
    let mut check = false;
    let mut write = false;
    for a in &args {
        match a.as_str() {
            "--check" => check = true,
            "--write" => write = true,
            "-h" | "--help" => {
                eprintln!(
                    "usage: skald-fmt [FILES...] [--check] [--write]\n\
                     \n\
                     No files: read stdin, write formatted output to stdout.\n\
                     --write:  overwrite each file with its formatted output.\n\
                     --check:  exit 1 if any file is not already formatted.\n\
                     --check and --write are mutually exclusive."
                );
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("unknown flag: {other}");
                return ExitCode::from(2);
            }
            other => files.push(other.to_string()),
        }
    }

    if check && write {
        eprintln!("error: --check and --write are mutually exclusive");
        return ExitCode::from(2);
    }

    if files.is_empty() {
        // Stdin mode: read all of stdin, format, write to stdout.
        let mut src = String::new();
        if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut src) {
            eprintln!("cannot read stdin: {e}");
            return ExitCode::from(2);
        }
        match skald_fmt::format_str(&src) {
            Ok(formatted) => {
                print!("{formatted}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(2)
            }
        }
    } else if check {
        // Check mode: report files that are not already formatted; exit 1 if any differ.
        let mut any_unformatted = false;
        for path in &files {
            let src = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("cannot read {path}: {e}");
                    return ExitCode::from(2);
                }
            };
            match skald_fmt::format_str(&src) {
                Ok(formatted) => {
                    if formatted != src {
                        eprintln!("{path}: not formatted");
                        any_unformatted = true;
                    }
                }
                Err(e) => {
                    eprintln!("{path}: {e}");
                    return ExitCode::from(2);
                }
            }
        }
        if any_unformatted {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        }
    } else if write {
        // Write mode: overwrite each file with its formatted output.
        for path in &files {
            let src = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("cannot read {path}: {e}");
                    return ExitCode::from(2);
                }
            };
            match skald_fmt::format_str(&src) {
                Ok(formatted) => {
                    if let Err(e) = std::fs::write(path, &formatted) {
                        eprintln!("cannot write {path}: {e}");
                        return ExitCode::from(2);
                    }
                }
                Err(e) => {
                    eprintln!("{path}: {e}");
                    return ExitCode::from(2);
                }
            }
        }
        ExitCode::SUCCESS
    } else {
        // Default (files, no flag): print each file's formatted output to stdout.
        for path in &files {
            let src = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("cannot read {path}: {e}");
                    return ExitCode::from(2);
                }
            };
            match skald_fmt::format_str(&src) {
                Ok(formatted) => print!("{formatted}"),
                Err(e) => {
                    eprintln!("{path}: {e}");
                    return ExitCode::from(2);
                }
            }
        }
        ExitCode::SUCCESS
    }
}
