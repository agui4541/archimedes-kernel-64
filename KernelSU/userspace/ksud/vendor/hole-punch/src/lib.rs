/*!
Utility methods for locating holes in sparse files
*/

use std::io::{Read, Seek};
use thiserror::Error;

cfg_if::cfg_if! {
    if #[cfg(any(target_os = "linux",
                 target_os = "android",
                 target_os = "freebsd",
    ))]{
        mod unix;
    } else if #[cfg(windows)] {
        mod windows;
    } else {
        mod default;
    }
}

#[cfg(test)]
mod test_utils;

#[derive(Error, Debug)]
pub enum ScanError {
    #[error("IO Error occurred")]
    IO(#[from] std::io::Error),
    #[error("An unknown error occurred interacting with the C API")]
    Raw(i32),
    #[error("The operation you are trying to perform is not supported on this platform")]
    UnsupportedPlatform,
    #[error("The filesystem does not support operating on sparse files")]
    UnsupportedFileSystem,
}

/// Flag for determining if a segment is a hole, or if it contains data
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SegmentType {
    Hole,
    Data,
}

/// Describes the location of a chunk in the file, as well as indicating if it
/// contains data or is a hole
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Marks this segment as either containing a hole, or containing data
    pub segment_type: SegmentType,
    pub start: u64,
    pub end: u64,
}

impl Segment {
    /// Returns true if the provided offset is within the range of bytes this
    /// segment specifies
    pub fn contains(&self, offset: u64) -> bool {
        offset >= self.start && offset <= self.end
    }

    /// Returns true if this segment is a Hole
    pub fn is_hole(&self) -> bool {
        self.segment_type == SegmentType::Hole
    }

    /// Returns true if this segment contains data
    pub fn is_data(&self) -> bool {
        self.segment_type == SegmentType::Data
    }
}

/// Trait for objects that can have sparsity
pub trait SparseFile: Read + Seek {
    /// Scans the file to find its logical chunks
    ///
    /// Will return a list of segments, ordered by their start position.
    ///
    /// The ranges generated are guaranteed to cover all bytes in the file, up
    /// to the last non-zero byte in the last segment containing data. All files
    /// are considered to have a single hole of indeterminate length at the end,
    /// and this library may not included that hole.
    ///
    /// `Hole` segments are guaranteed to represent a part of a file that does
    /// not contain any non-zero data, however, `Data` segments may represent
    /// parts of a file that contain what, logically, should be sparse segments.
    /// This is up to the mercy of your operating system and file system, please
    /// consult their documentation for how they handle sparse files for more
    /// details.
    ///
    /// Does not make any guarantee about maintaining the Seek position of the
    /// file, always seek back to a known point after calling this method.
    ///
    /// # Errors
    ///
    /// Will return `Err(ScanError::UnsupportedPlatform)` if support is not
    /// implemented for filesystem level hole finding on your system
    ///
    /// Will return `Err(ScanError::UnsupportedFileSystem)` if support is
    /// implemented for your operating system, but the filesystem does not
    /// support sparse files
    ///
    /// Will also return `Err` if any other I/O error occurs
    fn scan_chunks(&mut self) -> Result<Vec<Segment>, ScanError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use quickcheck_macros::quickcheck;

    // Creates a file based on desc, then tests that the resulting output of
    // file.scan_chunks() has every non-zero byte included
    fn test_covers_all_bytes(desc: SparseDescription) -> bool {
        let mut file = desc.to_file();
        // Get both sets of segments
        let input_segments = desc.segments();
        let output_segments = file
            .as_file_mut()
            .scan_chunks()
            .expect("Unable to scan chunks");
        println!("Output: \n {:?} \n", output_segments);
        // Find the last non-zero byte in the input segments
        let last_non_zero = input_segments
            .iter()
            .map(|x| {
                if let SegmentType::Data = x.segment_type {
                    x.end
                } else {
                    0
                }
            })
            .max()
            .unwrap_or(0);
        println!("Last non zero: {} \n", last_non_zero);
        let mut last_byte_touched = false;
        for (x, y) in output_segments.iter().zip(output_segments.iter().skip(1)) {
            if y.start != x.end + 1 {
                return false;
            }
            if y.end >= last_non_zero {
                println!("Last byte touched!");
                last_byte_touched = true;
            }
        }
        if output_segments.len() == 1 {
            if output_segments[0].end >= last_non_zero {
                println!("Last byte touched!");
                last_byte_touched = true;
            }
        }
        last_byte_touched || last_non_zero == 0
    }

    #[quickcheck]
    fn covers_all_bytes(desc: SparseDescription) -> bool {
        test_covers_all_bytes(desc)
    }

    // Constructs a file with desc, then verifies that the holes in the output
    // from file.scan_chunks() don't contain any data
    fn test_holes_have_no_data(desc: SparseDescription) -> bool {
        println!("Input: \n {:?} \n", desc);
        let mut file = desc.to_file();
        // Get both sets of segments
        let input_segments = desc.segments();
        let output_segments = file
            .as_file_mut()
            .scan_chunks()
            .expect("Unable to scan chunks");
        println!("Output: \n {:?} \n", output_segments);
        for segment in output_segments.iter().filter(|x| x.is_hole()) {
            if input_segments.iter().filter(|x| x.is_data()).any(|other| {
                let x = if segment.start > other.start {
                    !(segment.start > other.end)
                } else {
                    !(segment.end < other.start)
                };

                if x {
                    println!("Output {:?} overlaps Input {:?}", segment, other);
                }

                x
            }) {
                return false;
            }
        }
        true
    }

    #[quickcheck]
    fn holes_have_no_data(desc: SparseDescription) -> bool {
        test_holes_have_no_data(desc)
    }

    #[test]
    fn covers_all_bytes_failure_1() {
        let desc = SparseDescription::from_segments(vec![
            Segment {
                segment_type: SegmentType::Hole,
                start: 0,
                end: 3545867,
            },
            Segment {
                segment_type: SegmentType::Data,
                start: 3545868,
                end: 3625675,
            },
        ]);

        assert!(test_covers_all_bytes(desc));
    }

    #[test]
    fn covers_all_bytes_failure_2() {
        let desc = SparseDescription::from_segments(vec![Segment {
            segment_type: SegmentType::Hole,
            start: 0,
            end: 5440262,
        }]);

        assert!(test_covers_all_bytes(desc));
    }

    #[test]
    fn holes_have_no_data_failure_1() {
        let desc = SparseDescription::from_segments(vec![
            Segment {
                segment_type: SegmentType::Data,
                start: 0,
                end: 106392,
            },
            Segment {
                segment_type: SegmentType::Hole,
                start: 106393,
                end: 713195,
            },
            Segment {
                segment_type: SegmentType::Data,
                start: 713196,
                end: 1164291,
            },
            Segment {
                segment_type: SegmentType::Hole,
                start: 1164292,
                end: 1871333,
            },
            Segment {
                segment_type: SegmentType::Data,
                start: 1871334,
                end: 2351104,
            },
            Segment {
                segment_type: SegmentType::Hole,
                start: 2351105,
                end: 2478705,
            },
            Segment {
                segment_type: SegmentType::Data,
                start: 2478706,
                end: 2568019,
            },
            Segment {
                segment_type: SegmentType::Hole,
                start: 2568020,
                end: 3062343,
            },
            Segment {
                segment_type: SegmentType::Data,
                start: 3062344,
                end: 3285810,
            },
            Segment {
                segment_type: SegmentType::Hole,
                start: 3285811,
                end: 3793122,
            },
            Segment {
                segment_type: SegmentType::Data,
                start: 3793123,
                end: 4166168,
            },
            Segment {
                segment_type: SegmentType::Hole,
                start: 4166169,
                end: 4249362,
            },
            Segment {
                segment_type: SegmentType::Data,
                start: 4249363,
                end: 4283128,
            },
            Segment {
                segment_type: SegmentType::Hole,
                start: 4283129,
                end: 4597394,
            },
            Segment {
                segment_type: SegmentType::Data,
                start: 4597395,
                end: 5204961,
            },
            Segment {
                segment_type: SegmentType::Hole,
                start: 5204962,
                end: 5270535,
            },
            Segment {
                segment_type: SegmentType::Data,
                start: 5270536,
                end: 5274355,
            },
            Segment {
                segment_type: SegmentType::Hole,
                start: 5274356,
                end: 5471034,
            },
            Segment {
                segment_type: SegmentType::Data,
                start: 5471035,
                end: 5547210,
            },
        ]);
        assert!(test_holes_have_no_data(desc));
    }
}
