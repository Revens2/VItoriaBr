//! Write marks (`shader.rs` `SLOT_MARKS`, `crates/victory-gpu/src/marks.rs`):
//! every store and atomic to a buffer sets the bit of the 64 KiB page of
//! title memory it touches, so the host copies back only those pages.

use super::*;

/// [`super::lower`]: with marks when they are asked for ([`set_write_marks`]
/// in `shader.rs`), and as before for a program with a write that cannot be
/// marked (the host then copies back all it may write: `unmarked_writes`).
pub(super) fn lower(parts: &Parts) -> Result<Lowered, ShaderError> {
    if crate::emit::shader::write_marks() {
        if let Ok(l) = super::lower_marks(parts, true)
            && !l.marks_incomplete
        {
            return Ok(l);
        }
        let mut l = super::lower_marks(parts, false)?;
        l.marks_incomplete = true;
        return Ok(l);
    }
    super::lower_marks(parts, false)
}

impl<'a> Lower<'a> {
    /// `lo`, `hi` plus the byte count `x` (a value), with the carry.
    pub(super) fn address_add(&mut self, lo: V, hi: V, x: V) -> (V, V) {
        let c = self.cur;
        let i32t = self.s.t.i32;
        let sum = self.s.m.bin(c, Bin::Add, lo, x);
        let wrapped = self.s.m.cmp(c, pred::IULT, sum, lo);
        let one = self.s.m.cast(c, Cast::ZExt, wrapped, i32t);
        let hi = self.s.m.bin(c, Bin::Add, hi, one);
        (sum, hi)
    }

    /// Write marks (`shader.rs`, `SLOT_MARKS`): sets the bits of the pages
    /// of the title bytes `lo`, `hi` and `lo`, `hi` + `bytes` - 1, each
    /// only where still clear (most writes find their page marked: a load,
    /// not an atomic). Call inside the guard of the write it marks.
    pub(super) fn mark_pages(&mut self, lo: V, hi: V, bytes: u32) {
        use crate::emit::shader::{MARK_PAGES, SLOT_MARKS};
        if !self.s.marks {
            return;
        }
        let (last_lo, last_hi) = self.address_plus(lo, hi, (bytes.max(1) - 1) as i32);
        let i32t = self.s.t.i32;
        let mut pages = Vec::new();
        for (lo, hi) in [(lo, hi), (last_lo, last_hi)] {
            let c = self.cur;
            let k16 = self.s.m.i32(16);
            let low = self.s.m.bin(c, Bin::LShr, lo, k16);
            let k8 = self.s.m.i32(0xFF);
            let high = self.s.m.bin(c, Bin::And, hi, k8);
            let high = self.s.m.bin(c, Bin::Shl, high, k16);
            let page = self.s.m.bin(c, Bin::Or, low, high);
            // The last bit stands for any address past the bitmap.
            let last = self.s.m.i32(MARK_PAGES - 1);
            let page = self.s.op(c, "binary.i32", Attr::ReadNone, i32t, 40, &[page, last]);
            self.s.m.tag(page, Tag::Clamp);
            pages.push(page);
        }
        let bitmap = self.s.root_uav_handle(SLOT_MARKS);
        for (i, &page) in pages.iter().enumerate() {
            let c = self.cur;
            let (k5, k2, k31, one, zero) = (
                self.s.m.i32(5),
                self.s.m.i32(2),
                self.s.m.i32(31),
                self.s.m.i32(1),
                self.s.m.i32(0),
            );
            let word = self.s.m.bin(c, Bin::LShr, page, k5);
            let word = self.s.m.bin(c, Bin::Shl, word, k2);
            let bit = self.s.m.bin(c, Bin::And, page, k31);
            let bit = self.s.m.bin(c, Bin::Shl, one, bit);
            let (iu, mask, align) = (self.s.m.undef(i32t), self.s.m.i8(1), self.s.m.i32(4));
            let old = self.s.op(
                c,
                "rawBufferLoad.f32",
                Attr::ReadOnly,
                self.s.t.res_ret,
                139,
                &[bitmap, word, iu, mask, align],
            );
            let old = self.s.m.extract(c, old, 0);
            let old = self.as_int(old);
            let seen = self.s.m.bin(c, Bin::And, old, bit);
            let mut clear = self.s.m.cmp(c, pred::EQ, seen, zero);
            if i == 1 {
                // The same page twice: once.
                let other = self.s.m.cmp(c, pred::NE, page, pages[0]);
                clear = self.s.m.bin(c, Bin::And, clear, other);
            }
            let at = self.begin_guarded(Some(clear));
            // Operation 2 is OR.
            self.s.op(
                self.cur,
                "atomicBinOp.i32",
                Attr::NoUnwind,
                i32t,
                78,
                &[bitmap, k2, word, iu, iu, bit],
            );
            self.end_guarded(at, &[]);
        }
    }

    /// Write marks for `bytes` bytes written at byte `byte` of the view
    /// the descriptor `src` is: its title address from the host's word (a
    /// table entry at a fixed place, `USER_VIEW_ADDRESS`), its table by
    /// heap slot (`SLOT_VIEWS`) or, for a root view, from the host's word.
    /// A view this cannot name makes the translation incomplete.
    pub(super) fn mark_buffer(&mut self, src: &Source, byte: V, bytes: u32) {
        use crate::emit::shader::USER_ROOT_START;
        if !self.s.marks {
            return;
        }
        let start = match src {
            // A view in a table at a fixed place: the host's per-draw word
            // for its address (no pass over the heap's buffers). One the
            // program picks at run time: the host's table by heap slot.
            Source::Loaded { load, first } => match (*first == 0).then(|| self.view_address(*load)).flatten() {
                Some(address) => Some(address),
                None => match self.heap_index(*load, *first, false) {
                    Ok(slot) => {
                        let c = self.cur;
                        let r = self.s.read_view_slot(c, slot);
                        let lo = self.s.m.extract(c, r, 0);
                        let hi = self.s.m.extract(c, r, 1);
                        Some((self.as_int(lo), self.as_int(hi)))
                    }
                    Err(_) => None,
                },
            },
            Source::Built { address, .. } => match self.x.native.layout.get(address.param).copied() {
                Some(rp) if rp.root == RootType::Descriptor => Some((
                    self.host_word(RootType::Other(USER_ROOT_START), rp.slot, 0),
                    self.host_word(RootType::Other(USER_ROOT_START + 1), rp.slot, 0),
                )),
                _ => None,
            },
            _ => None,
        };
        let Some((lo, hi)) = start else {
            self.s.marks_incomplete = true;
            return;
        };
        let (lo, hi) = self.address_add(lo, hi, byte);
        self.mark_pages(lo, hi, bytes);
    }
}
