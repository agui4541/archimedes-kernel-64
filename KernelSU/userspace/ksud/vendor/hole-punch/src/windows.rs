use super::*;

use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::os::windows::io::{AsRawHandle, RawHandle};

use winapi::shared::minwindef::{DWORD, LPVOID};
use winapi::shared::ntdef::LARGE_INTEGER;
use winapi::um::fileapi::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
use winapi::um::ioapiset::DeviceIoControl;
use winapi::um::winioctl::FSCTL_QUERY_ALLOCATED_RANGES;
use winapi::um::winnt::FILE_ATTRIBUTE_SPARSE_FILE;

use std::mem::MaybeUninit;

struct Range {
    start: u64,
    end: u64,
}

impl SparseFile for File {
    fn scan_chunks(&mut self) -> std::result::Result<std::vec::Vec<Segment>, ScanError> {
        // Get the length before doing anything
        let len = self.seek(SeekFrom::End(0))?;
        // get the handle from the file
        let handle = self.as_raw_handle();
        // First check for an empty file
        if len == 0 {
            // Return nothing here, an empty file has no ranges
            Ok(vec![])
        } else if is_sparse(handle)? {
            // Call through and get the allocated ranges
            let ranges = get_allocated_ranges(handle, len)?;
            // the file isn't empty if we are here, so we should have at least one range
            assert!(ranges.len() >= 1);
            // Make a place to put our segments, and copy over our ranges
            let mut segments = ranges
                .iter()
                .map(|x| Segment {
                    segment_type: SegmentType::Data,
                    start: x.start,
                    end: x.end,
                })
                .collect::<Vec<_>>();
            // We need to fill in the sparse segments
            // First, check if the first
            // data segment starts at 0, otherwise we have to add a sparse
            // segment
            if ranges[0].start > 0 {
                segments.push(Segment {
                    segment_type: SegmentType::Hole,
                    start: 0,
                    end: ranges[0].start - 1,
                });
            }
            // Fill in the gaps
            for (before, after) in ranges.iter().zip(ranges.iter().skip(1)) {
                // Make sure there is a gap before proceeding, the documentation
                // for winapi is utter crap, and I can't tell if this is
                // actually something we need to do.
                if before.end + 1 < after.start {
                    segments.push(Segment {
                        segment_type: SegmentType::Hole,
                        start: before.end + 1,
                        end: after.start - 1,
                    });
                }
            }

            // Check to see if we need to add a hole segment at the end
            if ranges[ranges.len() - 1].end < len {
                segments.push(Segment {
                    segment_type: SegmentType::Hole,
                    start: ranges[ranges.len() - 1].end + 1,
                    end: len,
                });
            }

            // Sort the segments vec, since we really have just been adding
            // segments willy-nilly
            segments.sort_by_key(|x| x.start);

            Ok(segments)
        } else {
            Ok(vec![Segment {
                segment_type: SegmentType::Data,
                start: 0,
                end: len,
            }])
        }
    }
}

/// Get the portions of a file that contain data
fn get_allocated_ranges(handle: RawHandle, size: u64) -> Result<Vec<Range>, ScanError> {
    // Define some types
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct FileAllocatedRange {
        offset: LARGE_INTEGER,
        length: LARGE_INTEGER,
    }
    const LEN: usize = 1024;
    type FileAllocatedRangeBuffer = [MaybeUninit<FileAllocatedRange>; LEN];
    // Get the range to query
    // These are just integers under the hood, so we can use zeroed 'uninitialized' memory safely
    let mut offset: LARGE_INTEGER = unsafe { MaybeUninit::zeroed().assume_init() };
    let mut length: LARGE_INTEGER = unsafe { MaybeUninit::zeroed().assume_init() };
    // set offset to start of file for query range
    unsafe { *offset.QuadPart_mut() = 0 };
    // set length to provided length of file
    unsafe { *length.QuadPart_mut() = size as i64 };

    let mut query_range_buffer = FileAllocatedRange { offset, length };

    let mut buffer: FileAllocatedRangeBuffer = unsafe { MaybeUninit::uninit().assume_init() };
    let mut returned_bytes: DWORD = 0;

    let ret = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_QUERY_ALLOCATED_RANGES,
            &mut query_range_buffer as *mut _ as LPVOID,
            std::mem::size_of::<FileAllocatedRange>() as DWORD,
            &mut buffer as *mut _ as LPVOID,
            std::mem::size_of::<FileAllocatedRangeBuffer>() as DWORD,
            &mut returned_bytes,
            std::ptr::null_mut(),
        )
    };

    // Check the returned value
    // FIXME: WIll error if the user provides a massive file with too many ranges
    // Really need to check for MORE_DATA and do a loop
    if ret == 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    // Find out how many ranges we have
    let range_count: usize = returned_bytes as usize / std::mem::size_of::<FileAllocatedRange>();

    // Create a place to put our ranges
    let mut ranges: Vec<Range> = Vec::new();

    // Iterate through the buffer and extract ranges
    for i in 0..range_count {
        // Since we are only iterating up to the point DeviceIoControl returned, this unwrap is safe
        let range: FileAllocatedRange = unsafe { buffer[i].assume_init() };
        let start = unsafe { *range.offset.QuadPart() } as u64;
        let end = unsafe { *range.length.QuadPart() } as u64 + start;
        ranges.push(Range { start, end });
    }
    Ok(ranges)
}

/// Check if the file is sparse
///
/// This will allow us to skip the nonsense and return a single range if it isn't
fn is_sparse(handle: RawHandle) -> Result<bool, ScanError> {
    // Create a space for the file_info to go
    let mut file_info: MaybeUninit<BY_HANDLE_FILE_INFORMATION> = MaybeUninit::zeroed();
    // Make the call
    let ret = unsafe { GetFileInformationByHandle(handle, file_info.as_mut_ptr()) };
    // Check for an error and indicate if there was one
    if ret == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // Now that we have the file info, unwrap it, we would have returned by now if it was still uninitialized
    let file_info = unsafe { file_info.assume_init() };
    Ok(file_info.dwFileAttributes & FILE_ATTRIBUTE_SPARSE_FILE != 0)
}
