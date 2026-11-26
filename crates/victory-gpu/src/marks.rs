//! Write marks (`VICTORY_WRITE_MARKS=1`, default off). Shaders translated
//! with write marks (`victory_shader::emit::set_write_marks`) set the bit
//! of every 64 KiB page of title memory they store to, in a bitmap each
//! list owns (`victory_shader::emit::SLOT_MARKS`). Where a submission's
//! shaders may write a big range of a host copy, the copy-back compares
//! and copies only the pages marked, not the range it declared.
//!
//! Exactness first: a page written and not marked would be lost, so
//!
//! * a list whose draws include a shader that may store and has no marks
//!   (a store the translator could not mark) is unmarked: all its ranges
//!   count as marked, as without the switch;
//! * a copy none of whose declarations is known to a mark entry is
//!   compared whole;
//! * marks are only read, never cleared: a list's bitmap is zeroed when
//!   the list takes it and lives until the list is reset, so the marks of
//!   flights still in the air are only ever a superset of what is needed;
//! * copies in the card's memory keep their own compare (not narrowed).
//!
//! shortcut: the bytes before the submission (the snapshot, `Shadow::writing`)
//! are still taken for the whole declared range: the marks narrow the
//! compare and the copy-back, not that copy.

use std::sync::{Arc, Mutex, OnceLock};

use victory_shader::emit::{MARK_BYTES, MARK_PAGES};
use windows::Win32::Graphics::Direct3D12::ID3D12Resource;

use crate::host::host;

/// The 64 KiB page of title addresses.
pub(crate) const PAGE_SHIFT: u32 = 16;

/// Whether write marks are on: `VICTORY_WRITE_MARKS=1`. The first answer
/// also tells the translator (before any shader is translated: the
/// translation cache keys on it).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_WRITE_MARKS").is_some_and(|v| v == "1");
        victory_shader::emit::set_write_marks(on);
        if on {
            tracing::info!("write marks on: copy-backs compare only the pages shaders marked");
        }
        on
    })
}

/// A list's bitmap: [`MARK_PAGES`] bits, in memory the CPU maps and
/// shaders write.
pub(crate) struct MarkBuf {
    /// Keeps the mapping alive (none in tests).
    _buffer: Option<ID3D12Resource>,
    data: *mut u32,
    pub(crate) gpu: u64,
}

// SAFETY: the mapping belongs to the buffer the struct owns; the GPU writes
// it and the CPU only reads it (volatile) or, before the buffer is handed
// to a list, zeroes it.
unsafe impl Send for MarkBuf {}
// SAFETY: as above.
unsafe impl Sync for MarkBuf {}

static SPARE: Mutex<Vec<MarkBuf>> = Mutex::new(Vec::new());
/// Bitmaps kept for reuse at most.
const SPARE_MOST: usize = 64;

impl MarkBuf {
    /// A zeroed bitmap, kept from an earlier list or new.
    pub(crate) fn take() -> Option<Arc<MarkBuf>> {
        let spare = SPARE.lock().unwrap_or_else(|e| e.into_inner()).pop();
        let buf = match spare {
            Some(b) => b,
            None => {
                let (buffer, data) = host()?.shared_buffer(u64::from(MARK_BYTES))?;
                crate::host::name(&buffer, || "write marks".to_owned());
                // SAFETY: a live buffer.
                let gpu = unsafe { buffer.GetGPUVirtualAddress() };
                MarkBuf { _buffer: Some(buffer), data: data.cast::<u32>(), gpu }
            }
        };
        // SAFETY: the mapping holds MARK_BYTES; no list has this bitmap.
        unsafe { std::ptr::write_bytes(buf.data, 0, (MARK_BYTES / 4) as usize) };
        Some(Arc::new(buf))
    }

    /// Whether the bit of title page `page` is set.
    pub(crate) fn marked(&self, page: usize) -> bool {
        // Past the bitmap: unknown, so marked.
        if page >= MARK_PAGES as usize - 1 {
            return true;
        }
        // SAFETY: inside the mapping; the GPU may be writing: a bit missed
        // now belongs to a write not yet done, which is read again by the
        // flight that made it.
        let word = unsafe { self.data.add(page / 32).read_volatile() };
        word >> (page % 32) & 1 != 0
    }
}

/// Gives a bitmap back for reuse once nothing holds it.
pub(crate) fn give_back(buf: Arc<MarkBuf>) {
    if let Ok(buf) = Arc::try_unwrap(buf) {
        let mut spare = SPARE.lock().unwrap_or_else(|e| e.into_inner());
        if spare.len() < SPARE_MOST {
            spare.push(buf);
        }
    }
}

/// What a list has of write marks.
#[derive(Default)]
pub(crate) struct ListMarks {
    pub(crate) buf: Option<Arc<MarkBuf>>,
    /// Some draw of the list may store with no marks: its ranges are all
    /// taken for marked.
    pub(crate) broken: bool,
}

impl ListMarks {
    /// What the list's declared ranges are known by: its bitmap, or none
    /// (all marked).
    pub(crate) fn known(&self) -> Option<Arc<MarkBuf>> {
        if self.broken { None } else { self.buf.clone() }
    }
}

impl Drop for ListMarks {
    fn drop(&mut self) {
        if let Some(buf) = self.buf.take() {
            give_back(buf);
        }
    }
}

/// What one submission declared of a copy: the ranges (as declared, joined
/// as the submission's `back` has them) and, for each list that declared
/// some of them, its bitmap (none: all of that list's ranges count).
pub(crate) struct Entry {
    pub(crate) ranges: Vec<(usize, usize)>,
    /// The lists' ranges, each with its bitmap or none.
    pub(crate) lists: Vec<(Vec<(usize, usize)>, Option<Arc<MarkBuf>>)>,
}

/// The runs of `[from, to)` a copy's entries leave marked: the pages with
/// a bit in some list's bitmap (or declared by a list without marks) that
/// the list declared. No entry: the whole range.
pub(crate) fn marked_runs(entries: &[Entry], from: usize, to: usize) -> Vec<(usize, usize)> {
    if entries.is_empty() || from >= to {
        return vec![(from, to)];
    }
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let page = 1usize << PAGE_SHIFT;
    for entry in entries {
        for (ranges, buf) in &entry.lists {
            for &(a, e) in ranges {
                let (a, e) = (a.max(from), e.min(to));
                if a >= e {
                    continue;
                }
                match buf {
                    None => runs.push((a, e)),
                    Some(buf) => {
                        let mut p = a >> PAGE_SHIFT;
                        while p << PAGE_SHIFT < e {
                            if buf.marked(p) {
                                runs.push((a.max(p << PAGE_SHIFT), e.min((p << PAGE_SHIFT) + page)));
                            }
                            p += 1;
                        }
                    }
                }
            }
        }
    }
    runs.sort_unstable();
    let mut joined: Vec<(usize, usize)> = Vec::new();
    for (a, e) in runs {
        match joined.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(e),
            _ => joined.push((a, e)),
        }
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf_with(pages: &[usize]) -> Arc<MarkBuf> {
        let mut words = vec![0u32; (MARK_BYTES / 4) as usize].into_boxed_slice();
        for &p in pages {
            words[p / 32] |= 1 << (p % 32);
        }
        let data = Box::leak(words).as_mut_ptr();
        // A bitmap with no device behind it: only its mapping is read.
        Arc::new(MarkBuf { _buffer: None, data, gpu: 0 })
    }

    const P: usize = 1 << PAGE_SHIFT;

    #[test]
    fn no_entry_is_the_whole_range() {
        assert_eq!(marked_runs(&[], 5 * P, 9 * P), vec![(5 * P, 9 * P)]);
    }

    #[test]
    fn only_marked_pages_of_a_declared_range_stay() {
        let buf = buf_with(&[6, 7, 9]);
        let entry = Entry { ranges: vec![(5 * P, 10 * P)], lists: vec![(vec![(5 * P, 10 * P)], Some(buf))] };
        assert_eq!(marked_runs(&[entry], 5 * P, 10 * P), vec![(6 * P, 8 * P), (9 * P, 10 * P)]);
    }

    #[test]
    fn a_range_is_cut_at_its_own_edges_not_the_pages() {
        let buf = buf_with(&[5, 6]);
        let a = 5 * P + 100;
        let e = 6 * P + 50;
        let entry = Entry { ranges: vec![(a, e)], lists: vec![(vec![(a, e)], Some(buf))] };
        assert_eq!(marked_runs(&[entry], a, e), vec![(a, e)]);
    }

    #[test]
    fn a_list_without_marks_keeps_all_its_ranges() {
        let buf = buf_with(&[]);
        let entry = Entry {
            ranges: vec![(0, 4 * P)],
            lists: vec![(vec![(P, 2 * P)], Some(buf)), (vec![(3 * P, 4 * P)], None)],
        };
        assert_eq!(marked_runs(&[entry], 0, 4 * P), vec![(3 * P, 4 * P)]);
    }

    #[test]
    fn marks_of_another_list_do_not_cover_a_range_it_did_not_declare() {
        // Page 8 is marked, but only the other list declared it, and that
        // list's bitmap is where its bit was set.
        let a = buf_with(&[8]);
        let b = buf_with(&[]);
        let entry = Entry {
            ranges: vec![(7 * P, 9 * P)],
            lists: vec![(vec![(7 * P, 8 * P)], Some(a)), (vec![(8 * P, 9 * P)], Some(b))],
        };
        assert_eq!(marked_runs(&[entry], 7 * P, 9 * P), Vec::<(usize, usize)>::new());
    }
}
