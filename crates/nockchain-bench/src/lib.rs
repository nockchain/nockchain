//! Nockchain Bench - Benchmarking and memory profiling harness for Nockchain
//!
//! This crate provides tools for:
//! - Memory sampling and attribution (NockStack vs heap/file-backed mappings)
//! - Speed-of-light benchmarks (maximum throughput without network)

pub mod sampler;
pub mod speed_of_light;
