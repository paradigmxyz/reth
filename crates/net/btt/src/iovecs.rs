//! This crate provides a helper type for a slice of [`IoVec`]s for zero-copy
//! functionality to bound iovecs by a byte count and to advance the buffer
//! cursor after partial vectored IO.
//!
//! # Bounding input buffers
//!
//! This is most useful when writing to a portion of a file while ensuring
//! that the file is not extended if the input buffers are larger than the
//! length of the file slice, which in the torrent scenario may occur if the
//! file is not a multiple of the block size.
//!
//! If the total size of the buffers exceeds the max length, the buffers are
//! split such that the first half returned is the portion of the buffers
//! that can be written to the file, while the second half is the remainder
//! of the buffer, to be used later. If the size of the total size of the
//! buffers is smaller than or equal to the slice, this is essentially
//! a noop.
//!
//! In reality, the situation here is more complex than just splitting
//! buffers in half, but this is taken care of by the [`IoVecs`]
//! implementation.
//!
//! However, the abstraction leaks through because the iovec at which the
//! buffers were split may be shrunk to such a size as would enable all
//! buffers to stay within the file slice length. This can be restored using
//! [`IoVecs::into_tail`], but until this is called, the original
//! buffers cannot be used, which is enforced by the borrow checker.
//!
//! # Advancing the write cursor
//!
//! IO syscalls generally don't guarantee writing or filling the input buffers
//! in one system call. This is why these APIs always return the number of bytes
//! transferred, so that calling code can advance the buffer cursor. This
//! functionality implemented by the [`IoVecs::advance`] method, which takes
//! offsets the start of the slices by some number of bytes.
//!
//! # Examples
//!
//! What follows is a complete example making use of both above mentioned
//! API features.
//!
//! Visualized, this looks like the following:
//!
//! ```text
//! ------------------------------
//! | file slice: 25             |
//! -----------------------------------
//! | block: 16      | block: 16 ^    |
//! -----------------------------^-----
//!                              ^
//!                          split here
//! ```
//!
//! In this example, the first half of the split would be [0, 25), the
//! second half would be [25, 32).
//!
//! ```
//! use {
//!     std::{
//!         fs::{OpenOptions, File},
//!         os::unix::io::AsRawFd,
//!     },
//!     nix::sys::uio::{pwritev, IoVec},
//!     cratetorrent::iovecs::IoVecs,
//! };
//!
//! let file = OpenOptions::new().write(true).create(true).open("/tmp/example").unwrap();
//! let file_len = 25;
//! // total length of buffers is 32
//! let blocks =
//!     vec![(0..16).collect::<Vec<u8>>(), (16..32).collect::<Vec<u8>>()];
//! // the raw data is in vecs but file IO works with iovecs, so we need to make
//! // at least one allocation here to get a contiguouos memory region of iovecs
//! let mut bufs: Vec<_> =
//!     blocks.iter().map(|buf| IoVec::from_slice(&buf)).collect();
//! let mut iovecs = bufs.as_mut_slice();
//! // bound iovecs by the file length so that we don't write past its end
//! let mut write_bufs = IoVecs::bounded(iovecs, file_len);
//!
//! // get write buffers as an immutable slice of iovecs, which will return the
//! // first 25 bytes, and write the first 25 bytes of buffers
//! assert_eq!(write_bufs.as_slice().iter().map(|iov| iov.as_slice().len()).sum::<usize>(), file_len);
//! let write_count = write_vectored_at(&file, 0, &mut write_bufs);
//! assert_eq!(write_count, file_len);
//!
//! // after this is done, we can use the second half of the buffers
//! iovecs = write_bufs.into_tail();
//! assert_eq!(iovecs.iter().map(|iov| iov.as_slice().len()).sum::<usize>(), 32 - file_len);
//!
//! // now we can write the rest of the buffers to another file
//!
//! /// Writes the slice of iovecs to the file.
//! fn write_vectored_at<'a>(file: &File, mut offset: i64, iovecs: &mut IoVecs<'a>) -> usize {
//!     let mut total_write_count = 0;
//!     while !iovecs.as_slice().is_empty() {
//!         let write_count = pwritev(
//!             file.as_raw_fd(),
//!             iovecs.as_slice(),
//!             offset,
//!         )
//!         .expect("cannot write to file");
//!         iovecs.advance(write_count);
//!         total_write_count += write_count;
//!         offset += write_count as i64;
//!     }
//!     total_write_count
//! }
//! ```
//!
//! [`IoVec`]: https://docs.rs/nix/0.17.0/nix/sys/uio/struct.IoVec.html
//! [`IoVecs`]: ./struct.IoVecs.html
//! [`IoVecs::into_tail`]: ./struct.IoVecs.html#method.into_tail
//! [`IoVecs::advance`]: ./struct.IoVecs.html#method.advance

pub use nix::sys::uio::IoVec;

/// Wrapper over a slice of [`IoVec`]s that provides zero-copy functionality to
/// pass only a subslice of the iovecs to vectored IO functions.
///
/// [`IoVec`](https://docs.rs/nix/0.17.0/nix/sys/uio/struct.IoVec.html)
#[derive(Debug)]
pub struct IoVecs<'a> {
    /// The entire view of the underlying buffers.
    bufs: &'a mut [IoVec<&'a [u8]>],
    /// If set, the buffer is bounded by a given boundary, and is effectively
    /// "split". This includes metadata to reconstruct the second half of the
    /// split.
    split: Option<Split>,
}

impl<'a> IoVecs<'a> {
    /// Bounds the iovecs, potentially spliting it in two, if the total byte
    /// count of the buffers exceeds the limit.
    ///
    /// # Arguments
    ///
    /// * `bufs` - A slice that points to a contiguous list of IO vectors, which in turn point to
    ///   the actual blocks of memory used for file IO.
    /// * `max_len` - The maximum byte count of the total number of bytes in the IO vectors.
    ///
    /// # Panics
    ///
    /// The constructor panics if the max length is 0.
    pub fn bounded(bufs: &'a mut [IoVec<&'a [u8]>], max_len: usize) -> Self {
        assert!(max_len > 0, "IoVecs max length should be larger than 0");

        // Detect whether the total byte count in bufs exceeds the slice length
        // by accumulating the buffer lengths and stopping at the buffer whose
        // accumulated length exceeds the slice length. Taking the example in
        // the docs, this would mean that we stop at the second buffer.
        //
        // i | len | cond
        // --|-----|---------------
        // 0 | 16  | len >= 25 -> f
        // 1 | 32  | len >= 32 -> t
        //
        // Another edge case is when the file boundary is at a buffer boundary.
        //
        // -----------------------------------
        // | file slice: 32                  |
        // ----------------------------------------------------
        // | block: 16      | block: 16      | block: 16      |
        // ----------------------------------^-----------------
        //                                   ^
        //                               split here
        //
        // i | len | cond
        // --|-----|---------------
        // 0 | 16  | len >= 32 -> f
        // 1 | 32  | len >= 32 -> t
        //
        // TODO: Can we make use of the fact that blocks are of the same length,
        // except for potentially the last one? We could skip over the first
        // n blocks whose summed up size is still smaller than the file .
        let mut bufs_len = 0;
        let bufs_split_pos = match bufs.iter().position(|buf| {
            bufs_len += buf.as_slice().len();
            bufs_len >= max_len
        }) {
            Some(pos) => pos,
            None => return Self::unbounded(bufs),
        };

        // If we're here, it means that the total buffers length exceeds the
        // slice length and we must split the buffers.
        if bufs_len == max_len {
            // The buffer boundary aligns with the file boundary. There are two
            // cases here:
            // 1. the buffers are the same length as the file, in which case
            //    there is nothing to split,
            // 2. or we just need to split at the buffer boundary.
            if bufs_split_pos + 1 == bufs.len() {
                // the split position is the end of the last buffer, so there is
                // nothing to split
                Self::unbounded(bufs)
            } else {
                // we can split at the buffer boundary
                Self::split_at_buffer_boundary(bufs, bufs_split_pos)
            }
        } else {
            // Otherwise the buffer boundary does not align with the file
            // boundary (as in the doc example), so we must trim the iovec that
            // is at the file boundary.

            // Find the position where we need to split the iovec. We need the
            // relative offset in the buffer, which we can get by first getting
            // the absolute offset of the buffer withiin all buffers and then
            // subtracting that from the file length.
            let buf_to_split = bufs[bufs_split_pos].as_slice();
            let buf_offset = bufs_len - buf_to_split.len();
            let buf_split_pos = max_len - buf_offset;
            debug_assert!(buf_split_pos < buf_to_split.len());

            // TODO: should we calculate `buf_split_pos` inside this function?
            Self::split_within_buffer(bufs, bufs_split_pos, buf_split_pos)
        }
    }

    /// Creates an unbounded `IoVec`, meaning that no split is necessary.
    pub fn unbounded(bufs: &'a mut [IoVec<&'a [u8]>]) -> Self {
        Self { bufs, split: None }
    }

    /// Creates a "clean split", in which the split occurs at the buffer
    /// boundary and `bufs` need only be split at the slice level.
    fn split_at_buffer_boundary(bufs: &'a mut [IoVec<&'a [u8]>], pos: usize) -> Self {
        Self { bufs, split: Some(Split { pos, split_buf_second_half: None }) }
    }

    /// Creates a split where the split occurs within one of the buffers of
    /// `bufs`.
    fn split_within_buffer(
        bufs: &'a mut [IoVec<&'a [u8]>],
        split_pos: usize,
        buf_split_pos: usize,
    ) -> Self {
        // save the original slice at the boundary, so that later we can
        // restore it
        let buf_to_split = bufs[split_pos].as_slice();

        // trim the overhanging part off the iovec
        let (split_buf_first_half, split_buf_second_half) = buf_to_split.split_at(buf_split_pos);

        // We need to convert the second half of the split buffer into its
        // raw representation, as we can't store a reference to it as well
        // as store mutable references to the rest of the buffer in
        // `IoVecs`.
        // This is safe:
        // 1. The second half of the buffer is not used until the buffer
        //    is reconstructed.
        // 2. And we don't leak the raw buffer or pointers for other code to
        //    unsafely reconstruct the slice. The slice is only reconstructined
        //    in `IoVecs::into_second_half`, assigning it to the `IoVec` at
        //    `split_pos` in `bufs`, without touching its underlying memory.
        let split_buf_second_half =
            RawBuf { ptr: split_buf_second_half.as_ptr(), len: split_buf_second_half.len() };

        // Shrink the iovec at the file boundary:
        //
        // Here we need to use unsafe code as there is no way to borrow
        // a slice from `bufs` (`buf_to_split` above), and then assigning
        // that same slice to another element of bufs below, as that would
        // be an immutable and mutable borrow at the same time, breaking
        // aliasing rules.
        // (https://doc.rust-lang.org/std/primitive.slice.html#method.swap
        // doesn't work here as we're not simply swapping elements of within
        // the same slice.)
        //
        // However, it is safe to do so, as we're not actually touching the
        // underlying byte buffer that the slice refers to, but simply replacing
        // the `IoVec` at `split_pos` in `bufs`, i.e.  shrinking the slice
        // itself, not the memory region pointed to by the slice.
        let split_buf_first_half = unsafe {
            std::slice::from_raw_parts(split_buf_first_half.as_ptr(), split_buf_first_half.len())
        };
        bufs[split_pos] = IoVec::from_slice(split_buf_first_half);

        Self {
            bufs,
            split: Some(Split {
                pos: split_pos,
                split_buf_second_half: Some(split_buf_second_half),
            }),
        }
    }

    /// Returns an immutable slice to the iovecs in the first half of the split.
    #[inline]
    pub fn as_slice(&self) -> &[IoVec<&'a [u8]>] {
        if let Some(split) = &self.split {
            // due to `Self::advance` it may be that the first half off the
            // split is actually empty, in  which case we need to return an
            // empty slice
            if split.pos == 0 && !self.bufs.is_empty() && self.bufs[0].as_slice().is_empty() {
                &self.bufs[0..0]
            } else {
                // we need to include the buffer under the split position too
                &self.bufs[0..=split.pos]
            }
        } else {
            &self.bufs[..]
        }
    }

    /// Advances the internal cursor of the iovecs slice.
    ///
    /// # Notes
    ///
    /// Elements in the slice may be modified if the cursor is not advanced to
    /// the end of the slice. For example if we have a slice of buffers with 2
    /// `IoSlice`s, both of length 8, and we advance the cursor by 10 bytes the
    /// first `IoSlice` will be untouched however the second will be modified to
    /// remove the first 2 bytes (10 - 8).
    ///
    /// # Panics
    ///
    /// Panics if `n` is larger than the combined byte count of the
    /// buffers (first half of the split if there is any, or bytes in all
    /// buffers if there is not).
    #[inline]
    pub fn advance(&mut self, n: usize) {
        // This is mostly borrowed from:
        // https://doc.rust-lang.org/src/std/io/mod.rs.html#1108-1127
        //
        // However, there is quite a bit more complexity here due to the iovecs
        // potentially being split. For one, we must not advance past the split
        // boundary, and because the iovecs may be split either on a buffer
        // boundary or within a buffer, the edge cases crop up here.
        //
        // What we do is we first count how many whole buffers can be trimmed
        // off from `self.as_slice()` (which returns only the first half of the
        // split, if there is any), and then determine whether it is allowed to
        // advance the given amount of bytes.
        //
        // Assuming `n` is for the whole first part of the split (the edge cases
        // arise when near the split boundary), even if we're on a buffer
        // boundary, we can not just trim off all buffers up until the boundary,
        // because then, when returning the first half in `self.as_slice()` we
        // would not be able to determine whether to return an empty slice or
        // not. Either way, we need to keep the buffer at the split position, as
        // some bytes of that buffer are either in the second half of the split,
        // or the buffer is empty which we use as a marker for returning and
        // empty slice.
        //
        // TODO: Can we make this faster by assuming that all blocks (except for
        // potentially the last one) are the same length? This should be the
        // case for all blocks transferred within torrent (except for the last
        // piece's last block, if torrent is not a multiple of the block size),
        // but verify if this is correct.

        // the number of buffers to remove
        let mut bufs_to_remove_count = 0;
        // the total length of all the to be removed buffers
        let mut total_removed_len = 0;

        // count the whole buffers to remove
        for buf in self.as_slice().iter() {
            let buf_len = buf.as_slice().len();
            // if the last byte to be removed is in this buffer, don't remove
            // buffer, we just need to adjust its offset
            if total_removed_len + buf_len > n {
                break
            } else {
                // otherwise there are more bytes to remove than this buffer,
                // ergo we want to remove it
                total_removed_len += buf_len;
                bufs_to_remove_count += 1;
            }
        }

        // if there is a split and we want to trim off the whole first half, we
        // must keep the buffer at the split position
        if let Some(split) = &self.split {
            if bufs_to_remove_count == split.pos + 1 {
                if n > total_removed_len {
                    panic!("cannot advance iovecs by more than buffers length");
                }

                bufs_to_remove_count -= 1;
                total_removed_len -=
                    self.as_slice().last().map(|s| s.as_slice().len()).unwrap_or(0);
            }
        }

        // trim buffers off the front of `self.bufs`
        //
        // hack: We need the original lifetime of the slice and not the
        // reborrowed temporary lifetime of `&mut self` passed to this function
        // (as would happen by re-assigning a sublice of `self.bufs` to itself),
        // so we move the `bufs` slice out of `self` by value with the original
        // lifetime and take the slice from that.
        let bufs = std::mem::replace(&mut self.bufs, &mut []);
        self.bufs = &mut bufs[bufs_to_remove_count..];

        // if there is a split, also adjust the split position
        if let Some(split) = &mut self.split {
            if bufs_to_remove_count >= split.pos {
                split.pos = 0;
            } else {
                split.pos -= bufs_to_remove_count;
            }
        }

        // if there are buffers left, it may be that the first buffer needs some
        // bytes trimmed off its front
        if !self.bufs.is_empty() {
            // adjust the advance count
            let n = n - total_removed_len;
            if n > 0 {
                let slice = self.bufs[0].as_slice();
                assert!(slice.len() >= n);
                let ptr = slice.as_ptr();
                let slice = unsafe { std::slice::from_raw_parts(ptr.add(n), slice.len() - n) };
                self.bufs[0] = IoVec::from_slice(slice);
            }
        }
    }

    /// Returns the second half of the split, reconstructing the split buffer in
    /// the middle, if necessary, consuming the split in the process.
    #[inline]
    pub fn into_tail(self) -> &'a mut [IoVec<&'a [u8]>] {
        if let Some(second_half) = self.split {
            // If the buffer at the boundary was split, we need to restore it
            // first. Otherwise the buffers were split at a buffer boundary so
            // we can just return the second half of the split.
            if let Some(split_buf_second_half) = second_half.split_buf_second_half {
                // See notes in `Self::split_within_buffer`: the pointers here
                // refer to the same buffer at `bufs[split_pos]`, so all we're
                // doing is resizing the slice at that position to be the second
                // half of the original slice that was untouched since creating
                // this split.
                let split_buf_second_half = unsafe {
                    let slice = std::slice::from_raw_parts(
                        split_buf_second_half.ptr,
                        split_buf_second_half.len,
                    );
                    IoVec::from_slice(slice)
                };

                // restore the second half of the split buffer
                self.bufs[second_half.pos] = split_buf_second_half;
            }
            // return a slice to the buffers starting at the split position
            &mut self.bufs[second_half.pos..]
        } else {
            // otherwise there is no second half, so we return an empty slice
            let write_buf_len = self.bufs.len();
            &mut self.bufs[write_buf_len..]
        }
    }
}

/// Represents the second half of a `&mut [IoVec<&[u8]>]` split into two,
/// where the split may not be on the boundary of two buffers.
///
/// The complication arises from the fact that the split may not be on a buffer
/// boundary, but we want to perform the split by keeping the original
/// slices (i.e. without allocating a new vector). This requires keeping the
/// first part of the slice, the second part of the slice, and if the split
/// occurred within a buffer, a copy of the second half of that split buffer.
///
/// This way, the user can use the first half of the buffers to pass it for
/// vectored IO, without accidentally extending the file if it's too large, and
/// then reconstructing the second half of the split, using the copy of the
/// split buffer.
#[derive(Debug)]
struct Split {
    /// The position of the buffer in which the split occurred, either within
    /// the buffer or one past the end of the buffer. This means that this
    /// position includes the last buffer of the first half of the split, that
    /// is, we would split at `[0, pos]`.
    pos: usize,
    /// If set, it means that the buffer at `bufs[split_pos]` was further split
    /// in two. It contains the second half of the split buffer.
    split_buf_second_half: Option<RawBuf>,
}

/// A byte slice deconstructed into its raw parts.
#[derive(Debug)]
struct RawBuf {
    ptr: *const u8,
    len: usize,
}

/// This function is analogous to [`IoVecs::advance`], except that it works on a
/// list of mutable iovec buffers, while the former is for an immutable list of
/// such buffers.
///
/// The reason this is separate is because there is no need for the `IoVecs`
/// abstraction when working with vectored read IO: `preadv` only reads as much
/// from files as the buffers have capacity for. This is in fact symmetrical to
/// how `pwritev` works, which writes as much as is available in the buffers.
/// However, it has the effect that it may extend the file size, which is what
/// `IoVecs` guards against. Since this protection is not necessary for reads,
/// but advancing the buffer cursor is, a free function is available for this
/// purpose.
pub fn advance<'a>(bufs: &'a mut [IoVec<&'a mut [u8]>], n: usize) -> &'a mut [IoVec<&'a mut [u8]>] {
    // number of buffers to remove
    let mut bufs_to_remove_count = 0;
    // total length of all the to be removed buffers
    let mut total_removed_len = 0;

    for buf in bufs.iter() {
        let buf_len = buf.as_slice().len();
        // if the last byte to be removed is in this buffer, don't remove
        // buffer, we just need to adjust its offset
        if total_removed_len + buf_len > n {
            break
        } else {
            // otherwise there are more bytes to remove than this buffer,
            // ergo we want to remove it
            total_removed_len += buf_len;
            bufs_to_remove_count += 1;
        }
    }

    let bufs = &mut bufs[bufs_to_remove_count..];
    // if not all buffers were removed, check if we need to trim more bytes from
    // this buffer
    if !bufs.is_empty() {
        let buf = bufs[0].as_slice();
        let offset = n - total_removed_len;
        // TODO: document safety
        let slice = unsafe {
            std::slice::from_raw_parts_mut(buf.as_ptr().add(offset) as *mut u8, buf.len() - offset)
        };
        let _ = std::mem::replace(&mut bufs[0], IoVec::from_mut_slice(slice));
    }
    bufs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that splitting of the blocks that align with the file boundary at
    /// the last block is a noop.
    ///
    /// -----------------------------------
    /// | file slice: 32                  |
    /// -----------------------------------
    /// | block: 16      | block: 16      |
    /// -----------------------------------
    #[test]
    fn should_not_split_buffers_same_size_as_file() {
        let file_len = 32;
        let blocks = vec![(0..16).collect::<Vec<u8>>(), (16..32).collect::<Vec<u8>>()];
        let blocks_len: usize = blocks.iter().map(Vec::len).sum();

        let mut bufs: Vec<_> = blocks.iter().map(|buf| IoVec::from_slice(buf)).collect();
        let iovecs = IoVecs::bounded(&mut bufs, file_len);

        // we should have both buffers
        assert_eq!(iovecs.as_slice().len(), 2);
        // there was no split
        assert!(iovecs.split.is_none());

        // compare the contents of the first half of the split: convert it
        // to a flat vector for easier comparison
        let first_half: Vec<_> = iovecs.as_slice().iter().map(IoVec::as_slice).flatten().collect();
        // the expected first half has the same bytes as the blocks
        let expected_first_half: Vec<_> = blocks.iter().flatten().collect();
        assert_eq!(first_half.len(), file_len as usize);
        assert_eq!(first_half.len(), blocks_len);
        assert_eq!(first_half, expected_first_half);

        // restore the second half of the split buffer, which should be empty
        let second_half = iovecs.into_tail();
        assert!(second_half.is_empty());
    }

    /// Tests that splitting of the blocks whose combined length is smaller than
    /// that of the file is a noop.
    ///
    /// --------------------------------------------
    /// | file slice: 42                           |
    /// --------------------------------------------
    /// | block: 16      | block: 16      |
    /// -----------------------------------
    #[test]
    fn should_not_split_buffers_smaller_than_file() {
        let file_len = 42;
        let blocks = vec![(0..16).collect::<Vec<u8>>(), (16..32).collect::<Vec<u8>>()];
        let blocks_len: usize = blocks.iter().map(Vec::len).sum();

        let mut bufs: Vec<_> = blocks.iter().map(|buf| IoVec::from_slice(buf)).collect();
        let iovecs = IoVecs::bounded(&mut bufs, file_len);

        // we should have both buffers
        assert_eq!(iovecs.as_slice().len(), 2);
        // there was no split
        assert!(iovecs.split.is_none());

        // compare the contents of the first half of the split: convert it
        // to a flat vector for easier comparison
        let first_half: Vec<_> = iovecs.as_slice().iter().map(IoVec::as_slice).flatten().collect();
        // the expected first half has the same bytes as the blocks
        let expected_first_half: Vec<_> = blocks.iter().flatten().collect();
        assert_eq!(first_half.len(), blocks_len);
        assert_eq!(first_half, expected_first_half);

        // restore the second half of the split buffer, which should be empty
        let second_half = iovecs.into_tail();
        assert!(second_half.is_empty());
    }

    /// Tests splitting of the blocks that do not align with file boundary at the
    /// last block.
    ///
    /// ------------------------------
    /// | file slice: 25             |
    /// -----------------------------------
    /// | block: 16      | block: 16 ^    |
    /// -----------------------------^-----
    ///                              ^
    ///              split here into 9 and 7 long halves
    #[test]
    fn should_split_last_buffer_not_at_boundary() {
        let file_len = 25;
        let blocks = vec![(0..16).collect::<Vec<u8>>(), (16..32).collect::<Vec<u8>>()];

        let mut bufs: Vec<_> = blocks.iter().map(|buf| IoVec::from_slice(buf)).collect();
        let iovecs = IoVecs::bounded(&mut bufs, file_len);

        // we should have both buffers
        assert_eq!(iovecs.as_slice().len(), 2);

        // compare the contents of the first half of the split: convert it
        // to a flat vector for easier comparison
        let first_half: Vec<_> = iovecs.as_slice().iter().map(IoVec::as_slice).flatten().collect();
        // the expected first half is just the file slice number of bytes
        let expected_first_half: Vec<_> = blocks.iter().flatten().take(file_len as usize).collect();
        assert_eq!(first_half.len(), file_len as usize);
        assert_eq!(first_half, expected_first_half);

        // restore the second half of the split buffer
        let second_half = iovecs.into_tail();
        // compare the contents of the second half of the split: convert it
        // to a flat vector for easier comparison
        let second_half: Vec<_> = second_half.iter().map(IoVec::as_slice).flatten().collect();
        assert_eq!(second_half.len(), 7);
        // the expected second half is just the bytes after the file slice number of bytes
        let expected_second_half: Vec<_> =
            blocks.iter().flatten().skip(file_len as usize).collect();
        assert_eq!(second_half, expected_second_half);
    }

    /// Tests splitting of the blocks that do not align with file boundary.
    ///
    /// ------------------------------
    /// | file slice: 25             |
    /// ----------------------------------------------------
    /// | block: 16      | block: 16 ^    | block: 16      |
    /// -----------------------------^----------------------
    ///                              ^
    ///              split here into 9 and 7 long halves
    #[test]
    fn should_split_middle_buffer_not_at_boundary() {
        let file_len = 25;
        let blocks = vec![
            (0..16).collect::<Vec<u8>>(),
            (16..32).collect::<Vec<u8>>(),
            (32..48).collect::<Vec<u8>>(),
        ];

        let mut bufs: Vec<_> = blocks.iter().map(|buf| IoVec::from_slice(buf)).collect();
        let iovecs = IoVecs::bounded(&mut bufs, file_len);

        // we should have only the first two buffers
        assert_eq!(iovecs.as_slice().len(), 2);
        assert!(iovecs.split.is_some());

        // compare the contents of the first half of the split: convert it
        // to a flat vector for easier comparison
        let first_half: Vec<_> = iovecs.as_slice().iter().map(IoVec::as_slice).flatten().collect();
        // the expected first half is just the file slice number of bytes
        let expected_first_half: Vec<_> = blocks.iter().flatten().take(file_len as usize).collect();
        assert_eq!(first_half.len(), file_len as usize);
        assert_eq!(first_half, expected_first_half);

        // restore the second half of the split buffer
        let second_half = iovecs.into_tail();
        // compare the contents of the second half of the split: convert it to
        // a flat vector for easier comparison
        let second_half: Vec<_> = second_half.iter().map(IoVec::as_slice).flatten().collect();
        // the length should be the length of the second half the split buffer
        // as well as the remaining block's length
        assert_eq!(second_half.len(), 7 + 16);
        // the expected second half is just the bytes after the file slice number of bytes
        let expected_second_half: Vec<_> =
            blocks.iter().flatten().skip(file_len as usize).collect();
        assert_eq!(second_half, expected_second_half);
    }

    /// Tests that advancing only a fraction of the first half of the split does
    /// not affect the rest of the buffers.
    #[test]
    fn partial_advance_in_first_half_should_not_affect_rest() {
        let file_len = 25;
        let blocks = vec![
            (0..16).collect::<Vec<u8>>(),
            (16..32).collect::<Vec<u8>>(),
            (32..48).collect::<Vec<u8>>(),
        ];

        let mut bufs: Vec<_> = blocks.iter().map(|buf| IoVec::from_slice(buf)).collect();
        let mut iovecs = IoVecs::bounded(&mut bufs, file_len);

        // advance past the first buffer (less then the whole write buffer/file
        // length)
        let advance_count = 18;
        iovecs.advance(advance_count);

        // compare the contents of the first half of the split: convert it
        // to a flat vector for easier comparison
        let first_half: Vec<_> = iovecs.as_slice().iter().map(IoVec::as_slice).flatten().collect();
        // the expected first half is just the file slice number of bytes
        let expected_first_half: Vec<_> =
            blocks.iter().flatten().take(file_len as usize).skip(advance_count).collect();
        assert_eq!(first_half, expected_first_half);

        // restore the second half of the split buffer, which shouldn't be
        // affected by the above advance
        let second_half = iovecs.into_tail();
        // compare the contents of the second half of the split: convert it to
        // a flat vector for easier comparison
        let second_half: Vec<_> = second_half.iter().map(IoVec::as_slice).flatten().collect();
        // the length should be the length of the second half the split buffer
        // as well as the remaining block's length
        assert_eq!(second_half.len(), 7 + 16);
        // the expected second half is just the bytes after the file slice number of bytes
        let expected_second_half: Vec<_> =
            blocks.iter().flatten().skip(file_len as usize).collect();
        assert_eq!(second_half, expected_second_half);
    }

    /// Tests that advancing only a fraction of the first half of the split in
    /// multiple steps does not affect the rest of the buffers.
    #[test]
    fn advances_in_first_half_should_not_affect_rest() {
        let file_len = 25;
        let blocks = vec![
            (0..16).collect::<Vec<u8>>(),
            (16..32).collect::<Vec<u8>>(),
            (32..48).collect::<Vec<u8>>(),
        ];

        let mut bufs: Vec<_> = blocks.iter().map(|buf| IoVec::from_slice(buf)).collect();
        let mut iovecs = IoVecs::bounded(&mut bufs, file_len);

        // 1st advance past the first buffer
        let advance_count = 18;
        iovecs.advance(advance_count);

        // compare the contents of the first half of the split: convert it
        // to a flat vector for easier comparison
        let first_half: Vec<_> = iovecs.as_slice().iter().map(IoVec::as_slice).flatten().collect();
        // the expected first half is just the file slice number of bytes after
        // advancing
        let expected_first_half: Vec<_> =
            blocks.iter().flatten().take(file_len as usize).skip(advance_count).collect();
        assert_eq!(first_half, expected_first_half);

        // 2nd advance till the iovecs bound
        let advance_count = file_len - advance_count;
        iovecs.advance(advance_count);

        // the first half of the split should be empty
        let mut first_half = iovecs.as_slice().iter().map(IoVec::as_slice).flatten();
        assert!(first_half.next().is_none());

        // restore the second half of the split buffer, which shouldn't be
        // affected by the above advances
        let second_half = iovecs.into_tail();
        // compare the contents of the second half of the split: convert it to
        // a flat vector for easier comparison
        let second_half: Vec<_> = second_half.iter().map(IoVec::as_slice).flatten().collect();
        // the length should be the length of the second half the split buffer
        // as well as the remaining block's length
        assert_eq!(second_half.len(), 7 + 16);
        // the expected second half is just the bytes after the file slice number of bytes
        let expected_second_half: Vec<_> =
            blocks.iter().flatten().skip(file_len as usize).collect();
        assert_eq!(second_half, expected_second_half);
    }

    /// Tests that advancing the full write buffer advances only up to the first
    /// half of the split, that is at a buffer boundary, not affecting the second
    /// half.
    #[test]
    fn consuming_first_half_should_not_affect_second_half() {
        let file_len = 32;
        let blocks = vec![
            (0..16).collect::<Vec<u8>>(),
            (16..32).collect::<Vec<u8>>(),
            (32..48).collect::<Vec<u8>>(),
        ];

        let mut bufs: Vec<_> = blocks.iter().map(|buf| IoVec::from_slice(buf)).collect();
        let mut iovecs = IoVecs::bounded(&mut bufs, file_len);

        // advance past the first two buffers, onto the iovecs bound
        let advance_count = file_len;
        iovecs.advance(advance_count);

        // the first half of the split should be empty
        let mut first_half = iovecs.as_slice().iter().map(IoVec::as_slice).flatten();
        assert!(first_half.next().is_none());

        // restore the second half of the split buffer, which shouldn't be
        // affected by the above advance
        let second_half = iovecs.into_tail();
        // compare the contents of the second half of the split: convert it to
        // a flat vector for easier comparison
        let second_half: Vec<_> = second_half.iter().map(IoVec::as_slice).flatten().collect();
        // the length should be the length of the second half the split buffer
        // as well as the remaining block's length
        assert_eq!(second_half.len(), 16);
        // the expected second half is just the bytes after the file slice
        // number of bytes
        let expected_second_half: Vec<_> =
            blocks.iter().flatten().skip(file_len as usize).collect();
        assert_eq!(second_half, expected_second_half);
    }

    #[test]
    #[should_panic]
    fn should_panic_advancing_past_end() {
        let file_len = 32;
        let blocks = vec![
            (0..16).collect::<Vec<u8>>(),
            (16..32).collect::<Vec<u8>>(),
            (32..48).collect::<Vec<u8>>(),
        ];

        let mut bufs: Vec<_> = blocks.iter().map(|buf| IoVec::from_slice(buf)).collect();
        let mut iovecs = IoVecs::bounded(&mut bufs, file_len);

        let advance_count = file_len + 5;
        iovecs.advance(advance_count);
    }

    #[test]
    fn should_advance_into_first_buffer() {
        let mut bufs = vec![vec![0, 1, 2], vec![3, 4, 5]];
        let mut iovecs: Vec<_> = bufs.iter_mut().map(|b| IoVec::from_mut_slice(b)).collect();

        // should trim some from the first buffer
        let n = 2;
        let iovecs = advance(&mut iovecs, n);
        let actual: Vec<_> = iovecs.iter().map(|b| b.as_slice().to_vec()).flatten().collect();
        let expected: Vec<_> = bufs.iter().flatten().skip(n).copied().collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn should_trim_whole_first_buffer() {
        let mut bufs = vec![vec![0, 1, 2], vec![3, 4, 5], vec![6, 7, 8]];
        let mut iovecs: Vec<_> = bufs.iter_mut().map(|b| IoVec::from_mut_slice(b)).collect();

        // should trim entire first buffer
        let n = 3;
        let iovecs = advance(&mut iovecs, n);
        let actual: Vec<_> = iovecs.iter().map(|b| b.as_slice().to_vec()).flatten().collect();
        let expected: Vec<_> = bufs.iter().flatten().skip(n).copied().collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn should_advance_into_second_buffer() {
        let mut bufs = vec![vec![0, 1, 2], vec![3, 4, 5], vec![6, 7, 8]];
        let mut iovecs: Vec<_> = bufs.iter_mut().map(|b| IoVec::from_mut_slice(b)).collect();

        // should trim entire first buffer and some from second
        let n = 5;
        let iovecs = advance(&mut iovecs, n);
        let actual: Vec<_> = iovecs.iter().map(|b| b.as_slice().to_vec()).flatten().collect();
        let expected: Vec<_> = bufs.iter().flatten().skip(n).copied().collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn should_trim_all_buffers() {
        let mut bufs = vec![vec![0, 1, 2], vec![3, 4, 5], vec![6, 7, 8]];
        let mut iovecs: Vec<_> = bufs.iter_mut().map(|b| IoVec::from_mut_slice(b)).collect();

        // should trim everything
        let n = 9;
        let iovecs = advance(&mut iovecs, n);
        let mut actual = iovecs.iter().map(|b| b.as_slice().to_vec()).flatten();
        assert!(actual.next().is_none());
    }
}
