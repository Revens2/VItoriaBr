//! A select or carry-in on a mask the code holds as a number (from
//! FernandoHaeser/ztrx perf/30fps 427111f).

use super::{HashSet, Inst, Lower, MaskData, Operand, Reg, V, Value, Why};

impl<'a> Lower<'a> {
    /// Whether `v` is a phi that joins (directly or through other phis) a
    /// value structurisation tracks as a lane mask.
    pub(super) fn joins_lane_mask(&self, v: Value) -> bool {
        let st = self.x.st;
        let (mut work, mut seen) = (vec![v], HashSet::new());
        while let Some(v) = work.pop() {
            if !seen.insert(v) {
                continue;
            }
            for phi in self.x.p.blocks.iter().flat_map(|b| &b.phis) {
                if phi.value == v {
                    for &a in &phi.args {
                        if st.masks.contains_key(&a) || st.highs.contains_key(&a) {
                            return true;
                        }
                        work.push(a);
                    }
                }
            }
        }
        false
    }

    /// The selector or carry-in `src2` of a VALU instruction that
    /// structurisation does not follow as a lane mask (a constant bit
    /// pattern: every other lane, every other pair...; one loaded or built
    /// from scalars): this lane's bit of the number. Exact where host lanes
    /// are the console's (compute; `selects_on_untracked_mask` turns console
    /// lanes on for such a program).
    pub(super) fn number_mask_bit(&mut self, inst: &Inst, v: &crate::gfx10::Valu, c: usize, why: Why) -> Result<V, Why> {
        if self.mask_data != MaskData::Console || !self.console_lanes {
            return Err(why);
        }
        // Each host wave taken as a wave of its own has no console lane
        // numbers to pick the bits by.
        if self.local {
            return Err("a constant lane mask in a half-wave");
        }
        let Some(&Operand::Scalar { code, .. }) = v.src.get(2) else {
            return Err(why);
        };
        let m = inst.read(Reg::S(code)).ok_or(why)?;
        let lo = self.i32_of(m).map_err(|_| why)?;
        // wave64: the register after it holds lanes 32-63.
        let hi = if self.x.native.wave64 {
            let h = inst.read(Reg::S(code + 1)).ok_or(why)?;
            Some(self.i32_of(h).map_err(|_| why)?)
        } else {
            None
        };
        self.cross_lane(false)?;
        Ok(self.lane_bit(c, lo, hi))
    }
}
