//! Command lists' waits for memory values: how the title's queues wait for
//! one another (`list::wait_value`).

/// A wait until the value at `address` compares as asked to `reference`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Wait {
    pub(crate) address: usize,
    /// A `D3D12_COMPARISON_FUNC`.
    pub(crate) comparison: u32,
    pub(crate) reference: u64,
    pub(crate) bytes: usize,
}

impl Wait {
    /// Whether `value` ends the wait.
    pub(crate) fn met(&self, value: u64) -> bool {
        match self.comparison {
            1 => false,
            2 => value < self.reference,
            3 => value == self.reference,
            4 => value <= self.reference,
            5 => value > self.reference,
            6 => value != self.reference,
            7 => value >= self.reference,
            _ => true,
        }
    }

    pub(crate) fn value(&self) -> u64 {
        if self.address < 0x10000 {
            return 0;
        }
        // SAFETY: the title names readable GPU memory (unified with ours).
        unsafe {
            if self.bytes == 4 {
                u64::from((self.address as *const u32).read_unaligned())
            } else {
                (self.address as *const u64).read_unaligned()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_compare_as_asked() {
        let met = |comparison| {
            let wait = Wait {
                address: 0,
                comparison,
                reference: 5,
                bytes: 8,
            };
            [4, 5, 6].map(|v| wait.met(v))
        };
        assert_eq!(met(1), [false; 3]);
        assert_eq!(met(2), [true, false, false]);
        assert_eq!(met(3), [false, true, false]);
        assert_eq!(met(4), [true, true, false]);
        assert_eq!(met(5), [false, false, true]);
        assert_eq!(met(6), [true, false, true]);
        assert_eq!(met(7), [false, true, true]);
        assert_eq!(met(8), [true; 3]);
    }
}
