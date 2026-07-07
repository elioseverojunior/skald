// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::process::Command;

fn main() {
    // Get Rustc version - fallback to rustc --version if env var not available
    let rustc_version = std::env::var("RUSTC_VERSION")
        .or_else(|_| {
            Command::new("rustc")
                .arg("--version")
                .output()
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        })
        .unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=RUSTC_VERSION={rustc_version}");

    // Generate build timestamp
    let timestamp = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string();
    println!("cargo:rustc-env=BUILD_TIMESTAMP={timestamp}");

    // Get target triple - use environment variables with fallbacks
    let target = std::env::var("TARGET")
        .or_else(|_| std::env::var("CARGO_BUILD_TARGET"))
        .unwrap_or_else(|_| {
            // Determine target based on host architecture
            match std::env::consts::ARCH {
                "x86_64" => "x86_64-unknown-linux-gnu".to_string(),
                "aarch64" => "aarch64-unknown-linux-gnu".to_string(),
                arch => format!("{arch}-unknown-linux-gnu"),
            }
        });

    println!("cargo:rustc-env=TARGET={target}");

    // Re-run if these environment variables change
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-env-changed=CARGO_BUILD_TARGET");
    println!("cargo:rerun-if-env-changed=RUSTC_VERSION");
}
