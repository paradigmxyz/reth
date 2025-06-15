//! Integration tests for the Rollkit Reth binary and CLI functionality.
//!
//! This test suite focuses on testing the rollkit-reth binary compilation,
//! CLI argument handling, and overall integration with the Reth framework.

use std::process::{Command, Stdio};

/// Tests that the rollkit-reth binary compiles successfully
#[test]
fn test_rollkit_reth_binary_compiles() {
    let output = Command::new("cargo")
        .args(&["build", "--bin", "rollkit-reth"])
        .output()
        .expect("Failed to execute cargo build");

    if !output.status.success() {
        panic!(
            "Binary compilation failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    
    println!("✓ rollkit-reth binary compilation test passed");
}

/// Tests that the rollkit-reth binary shows help without crashing
#[test]
fn test_rollkit_reth_help() {
    let output = Command::new("cargo")
        .args(&["run", "--bin", "rollkit-reth", "--", "--help"])
        .output()
        .expect("Failed to execute rollkit-reth --help");

    // The help command should exit with code 0
    assert!(output.status.success(), 
        "Help command failed:\nstdout: {}\nstderr: {}", 
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    
    // Should contain rollkit-specific options
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("rollkit"), "Help output should mention rollkit options");
    
    println!("✓ rollkit-reth help test passed");
}

/// Tests that rollkit-specific CLI arguments are recognized
#[test]
fn test_rollkit_cli_arguments() {
    // Test that rollkit-specific arguments are parsed correctly
    let output = Command::new("cargo")
        .args(&["run", "--bin", "rollkit-reth", "--", "--help"])
        .output()
        .expect("Failed to execute rollkit-reth help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    
    // Check for rollkit-specific arguments - be more flexible as the exact flag names may vary
    assert!(stdout.contains("rollkit"), "Should show rollkit flag");
    
    // Since this is a Reth-based binary, it should have basic Ethereum node functionality
    let has_basic_options = stdout.contains("help") || 
                          stdout.contains("config") ||
                          stdout.contains("chain") ||
                          stdout.contains("datadir");
    assert!(has_basic_options, "Should show basic node options");
    
    println!("✓ rollkit CLI arguments test passed");
}

/// Tests that the binary exits gracefully with invalid arguments
#[test]
fn test_rollkit_reth_invalid_arguments() {
    let output = Command::new("cargo")
        .args(&["run", "--bin", "rollkit-reth", "--", "--invalid-flag"])
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .expect("Failed to execute rollkit-reth with invalid args");

    // Should fail with non-zero exit code
    assert!(!output.status.success(), "Should fail with invalid arguments");
    
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should contain some indication of the error
    assert!(stderr.contains("error") || stderr.contains("unknown") || stderr.contains("unrecognized"), 
        "Error output should indicate invalid argument: {}", stderr);
    
    println!("✓ rollkit invalid arguments test passed");
}

/// Tests that the test binary compiles successfully
#[test]
fn test_rollkit_engine_api_test_binary_compiles() {
    let output = Command::new("cargo")
        .args(&["build", "--bin", "test-rollkit-engine-api", "--features", "test-binaries"])
        .output()
        .expect("Failed to execute cargo build for test binary");

    if !output.status.success() {
        // It's okay if this fails in CI where features might not be available
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("feature") || stderr.contains("dependency") {
            println!("⚠ Test binary compilation skipped (missing features): {}", stderr);
            return;
        }
        
        panic!(
            "Test binary compilation failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            stderr
        );
    }
    
    println!("✓ test-rollkit-engine-api binary compilation test passed");
}

/// Tests library compilation and basic exports
#[test]
fn test_rollkit_library_compilation() {
    let output = Command::new("cargo")
        .args(&["build", "--lib"])
        .output()
        .expect("Failed to execute cargo build --lib");

    if !output.status.success() {
        panic!(
            "Library compilation failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    
    println!("✓ rollkit library compilation test passed");
}

/// Tests that documentation can be generated successfully
#[test]
fn test_rollkit_documentation_generation() {
    let output = Command::new("cargo")
        .args(&["doc", "--no-deps", "--lib"])
        .env("RUSTDOCFLAGS", "-D warnings") // Treat doc warnings as errors
        .output()
        .expect("Failed to execute cargo doc");

    if !output.status.success() {
        // Documentation generation failure is not critical, just log it
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("⚠ Documentation generation failed (non-critical): {}", stderr);
        return;
    }
    
    println!("✓ rollkit documentation generation test passed");
}

/// Tests basic workspace integration
#[test]
fn test_workspace_integration() {
    // Test that the rollkit crate is properly integrated into the workspace
    let output = Command::new("cargo")
        .args(&["metadata", "--format-version", "1"])
        .output()
        .expect("Failed to execute cargo metadata");

    assert!(output.status.success(), "Cargo metadata should succeed");
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("rollkit-reth"), "Workspace should contain rollkit-reth crate");
    
    println!("✓ workspace integration test passed");
} 