// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // parse: <data> --schema <schema> [--fix]
    let mut data_path: Option<String> = None;
    let mut schema_path: Option<String> = None;
    let mut fix = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--schema" | "-s" => schema_path = it.next().cloned(),
            "--fix" => fix = true,
            "-h" | "--help" => {
                eprintln!("usage: skald-validate <data.yaml> --schema <schema.yaml> [--fix]");
                return ExitCode::SUCCESS;
            }
            other if data_path.is_none() => data_path = Some(other.to_string()),
            other => {
                eprintln!("unexpected argument: {other}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(data_path), Some(schema_path)) = (data_path, schema_path) else {
        eprintln!("usage: skald-validate <data.yaml> --schema <schema.yaml> [--fix]");
        return ExitCode::from(2);
    };
    let data = match std::fs::read_to_string(&data_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {data_path}: {e}");
            return ExitCode::from(2);
        }
    };
    let schema = match std::fs::read_to_string(&schema_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {schema_path}: {e}");
            return ExitCode::from(2);
        }
    };

    if fix {
        let (fixed, report) = skald_validate::fix_str(&data, &schema);
        if let Err(e) = std::fs::write(&data_path, &fixed) {
            eprintln!("cannot write {data_path}: {e}");
            return ExitCode::from(2);
        }
        eprintln!("{report}");
        return ExitCode::SUCCESS;
    }

    let diags = skald_validate::validate_str(&data, &schema);
    if diags.is_empty() {
        eprintln!("{data_path}: valid");
        ExitCode::SUCCESS
    } else {
        for d in &diags {
            eprintln!(
                "{data_path}:{}:{}: {} ({})",
                d.line,
                d.column,
                d.message,
                if d.path.is_empty() { "/" } else { &d.path }
            );
        }
        ExitCode::FAILURE
    }
}
