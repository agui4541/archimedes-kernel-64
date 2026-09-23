//! Default fall back implementation for SparseFile trait on unsupported
//! platforms
//!
//! By defualt this will just error out
use super::*;

use std::fs::File;

impl SparseFile for File {
    fn scan_chunks(&mut self) -> std::result::Result<std::vec::Vec<Segment>, ScanError> {
        Err(ScanError::UnsupportedPlatform)
    }
}
