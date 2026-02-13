//! Integration tests for bridge file logging functionality.
//!
//! These tests verify the file logging infrastructure works correctly,
//! including directory creation, file rotation, and content writing.

#![allow(clippy::unwrap_used)]

use std::io::Write;

use tempfile::TempDir;
use tracing_appender::rolling::{RollingFileAppender, Rotation};

#[test]
fn test_log_directory_creation() {
    let temp = TempDir::new().unwrap();
    let log_dir = temp.path().join("logs");

    // Log dir should not exist yet
    assert!(!log_dir.exists());

    // Create it (same as init_bridge_tracing does)
    std::fs::create_dir_all(&log_dir).unwrap();
    assert!(log_dir.exists());
    assert!(log_dir.is_dir());
}

#[test]
fn test_rolling_file_appender_creates_file() {
    let temp = TempDir::new().unwrap();
    let log_dir = temp.path().join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();

    // Create a rolling appender (same config as bridge uses)
    let mut appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "bridge.log");

    // Write a test line
    writeln!(appender, "test log line").unwrap();
    drop(appender); // Flush and close

    // Verify file was created
    let entries: Vec<_> = std::fs::read_dir(&log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "Expected exactly one log file");

    // Verify filename pattern (bridge.log.YYYY-MM-DD)
    let filename = entries[0].file_name();
    let filename = filename.to_str().unwrap();
    assert!(
        filename.starts_with("bridge.log"),
        "Log file should start with 'bridge.log', got: {}",
        filename
    );
}

#[test]
fn test_log_content_written_correctly() {
    let temp = TempDir::new().unwrap();
    let log_dir = temp.path().join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();

    let mut appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "bridge.log");

    // Write multiple lines
    writeln!(appender, "first line").unwrap();
    writeln!(appender, "second line with data: 12345").unwrap();
    writeln!(appender, "third line").unwrap();
    drop(appender);

    // Read back and verify content
    let entries: Vec<_> = std::fs::read_dir(&log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    let log_path = entries[0].path();
    let content = std::fs::read_to_string(log_path).unwrap();

    assert!(content.contains("first line"), "Missing first line");
    assert!(
        content.contains("second line with data: 12345"),
        "Missing second line"
    );
    assert!(content.contains("third line"), "Missing third line");
}

#[test]
fn test_multiple_writes_append() {
    let temp = TempDir::new().unwrap();
    let log_dir = temp.path().join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();

    // First write
    {
        let mut appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "bridge.log");
        writeln!(appender, "write 1").unwrap();
    }

    // Second write (should append, not overwrite)
    {
        let mut appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "bridge.log");
        writeln!(appender, "write 2").unwrap();
    }

    // Verify both writes are present
    let entries: Vec<_> = std::fs::read_dir(&log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    let log_path = entries[0].path();
    let content = std::fs::read_to_string(log_path).unwrap();

    assert!(content.contains("write 1"), "Missing first write");
    assert!(content.contains("write 2"), "Missing second write");
}

#[test]
fn test_log_directory_with_nested_path() {
    let temp = TempDir::new().unwrap();
    let log_dir = temp.path().join("deep").join("nested").join("logs");

    // Should be able to create nested directories
    std::fs::create_dir_all(&log_dir).unwrap();
    assert!(log_dir.exists());

    // And write logs there
    let mut appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "bridge.log");
    writeln!(appender, "nested test").unwrap();
    drop(appender);

    let entries: Vec<_> = std::fs::read_dir(&log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);
}
