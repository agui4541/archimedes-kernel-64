//! lseek based implementation that uses `SEEK_DATA` and `SEEK_HOLE` to
//! reconstruct which segments of the file are data or holes
use super::*;

use std::fs::File;
use std::io::Error;
use std::os::unix::io::AsRawFd;

use errno::errno;
use libc::{c_int, lseek, off_t, EINVAL, ENXIO, SEEK_DATA, SEEK_END, SEEK_HOLE};

#[derive(Debug, Clone, Copy)]
enum Tag {
    Data(i64),
    Hole(i64),
    End(i64),
}

impl Tag {
    fn offset(&self) -> i64 {
        match self {
            Tag::Data(x) | Tag::Hole(x) | Tag::End(x) => *x,
        }
    }
}

impl SparseFile for File {
    fn scan_chunks(&mut self) -> std::result::Result<std::vec::Vec<Segment>, ScanError> {
        // Create our output vec
        let mut tags: Vec<Tag> = Vec::new();
        // Extract the raw fd from the file
        let fd = self.as_raw_fd();
        // Find the end
        let end = find_end(fd)?;
        // Our seeking loop assumes that we know what type the previous segment
        // is, so we check for the case where there is a hole at the start of
        // the file. This also does double duty checking for sparseness, as if
        // there are no holes, find_next_hole will return None, and we can short
        // circuit.
        if let Some(first_hole) = find_next_hole(fd, 0)? {
            let mut last_offset;
            if first_hole == 0 {
                last_offset = Tag::Hole(0);
            } else {
                last_offset = Tag::Data(0);
            }
            while last_offset.offset() < end {
                tags.push(last_offset);
                match last_offset {
                    Tag::Data(x) => {
                        // If the last tag was a data, we are looking for a hole
                        if let Some(next_offset) = find_next_hole(fd, x)? {
                            last_offset = Tag::Hole(next_offset);
                        } else {
                            // We know the last segment was a data, and there
                            // are no remaining holes, so we must be at the end
                            // of the file, so we end the loop and push an end
                            last_offset = Tag::End(end);
                        }
                    }
                    Tag::Hole(x) => {
                        // If the last tag was a hole, we are looking for a data
                        if let Some(next_offset) = find_next_data(fd, x)? {
                            last_offset = Tag::Data(next_offset);
                        } else {
                            // We know the last segment was a hole, and there
                            // are no remaining holes, so we must be at the end
                            // of the file, so we end the loop and push an end
                            last_offset = Tag::End(end);
                        }
                    }
                    // We never set last_offset to Tag::End until we are done
                    // with the loop, so if we encounter an End, we have made a
                    // major programming error
                    Tag::End(_) => unreachable!(),
                }
            }
            tags.push(Tag::End(end));
        } else {
            // In this situation, we have no holes in the data, so we just
            // represent a single data segment
            tags.push(Tag::Data(0));
            let end = find_end(fd)?;
            tags.push(Tag::End(end));
        }

        println!("{:?}", tags);

        // Process our list of start point tags into a list of segments.
        let mut tag_pairs = tags
            .iter()
            .copied()
            .zip(tags.iter().skip(1).copied())
            .map(|(x, y)| {
                // All these casts are valid, as the wrapper methods we use
                // around lseek will return Err rather than returning a value
                // less than 0
                match x {
                    Tag::Data(start) => Segment {
                        segment_type: SegmentType::Data,
                        start: start as u64,
                        end: (y.offset() - 1) as u64,
                    },
                    Tag::Hole(start) => Segment {
                        segment_type: SegmentType::Hole,
                        start: start as u64,
                        end: (y.offset() - 1) as u64,
                    },
                    // End should only ever be the last element the tag vector,
                    // so it can never be the first element of a pair
                    Tag::End(_) => unreachable!(),
                }
            })
            .collect::<Vec<_>>();
        // Modify the last element so it actually ends on the final offset
        let len = tag_pairs.len();
        tag_pairs[len - 1].end = end as u64;
        Ok(tag_pairs)
    }
}

fn find_next_hole(fd: c_int, offset: i64) -> Result<Option<i64>, ScanError> {
    unsafe {
        // First, call lseek with our file descriptor and current offset
        let new_offset = lseek(fd, offset as off_t, SEEK_HOLE);
        // if the return value of lseek is less than 0, an error has occurred
        if new_offset < 0 {
            // find and deref errno, honestly the scariest thing we do here
            let errno = errno().into();
            // Some of the errors we might not get here need to be handled
            // specially, and one of them isn' actually an error
            match errno {
                /// EINVAL indicates that the file system does not support
                /// SEEK_HOLE or SEEK_DATA, so we indicate as such
                EINVAL => Err(ScanError::UnsupportedFileSystem),
                // ENXIO indicates that the the file offset we are looking for
                // either doesn't exist, or would be beyond the end of the file.
                // In our case, this just means there is no next segment, so we
                // return Ok(none) to indicate as such.
                ENXIO => Ok(None),
                // None of the other error codes require special handling, so we
                // just turn them into an std::io::Error for user friendliness
                _ => Err(Error::last_os_error().into()),
            }
        } else {
            // If no errors occurred, we are good to return our offset.
            Ok(Some(new_offset as i64))
        }
    }
}

fn find_next_data(fd: c_int, offset: i64) -> Result<Option<i64>, ScanError> {
    unsafe {
        // First, call lseek with our file descriptor and current offset
        let new_offset = lseek(fd, offset as off_t, SEEK_DATA);
        // if the return value of lseek is less than 0, an error has occurred
        if new_offset < 0 {
            // find and deref errno, honestly the scariest thing we do here
            let errno = errno().into();
            // Some of the errors we might not get here need to be handled
            // specially, and one of them isn' actually an error
            match errno {
                /// EINVAL indicates that the file system does not support
                /// SEEK_HOLE or SEEK_DATA, so we indicate as such
                EINVAL => Err(ScanError::UnsupportedFileSystem),
                // ENXIO indicates that the the file offset we are looking for
                // either doesn't exist, or would be beyond the end of the file.
                // In our case, this just means there is no next segment, so we
                // return Ok(none) to indicate as such.
                ENXIO => Ok(None),
                // None of the other error codes require special handling, so we
                // just turn them into an std::io::Error for user friendliness
                _ => Err(Error::last_os_error().into()),
            }
        } else {
            // If no errors occurred, we are good to return our offset.
            Ok(Some(new_offset as i64))
        }
    }
}

fn find_end(fd: c_int) -> Result<i64, ScanError> {
    unsafe {
        let new_offset = lseek(fd, 0, SEEK_END);
        if new_offset < 0 {
            Err(Error::last_os_error().into())
        } else {
            Ok(new_offset as i64)
        }
    }
}
