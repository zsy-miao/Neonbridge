//! `VICTORY_READFIX=1` (off by default; with the relay and
//! `VICTORY_SYNC_PRECISE=1` or `VICTORY_RELAY=2`): a refresh of a host copy
//! that overlaps what another copy's flights in the air may write is served
//! on the card instead of waiting for those flights to land.
//!
//! The wait it replaces (`WHY_READ`, "title memory read ... another copy"):
//! a submission refreshes `[from, to)` of copy Y from title memory, and a
//! flight in the air declared shader writes over some of those bytes on
//! another copy X (an alias: a 4 MiB window over a title buffer, two views
//! of one buffer). Title memory there is not what the card will hold until
//! X's flight has landed and been copied back, so the submission waited for
//! that whole landing - and the ray-tracing builder declares every copy it
//! numbered written on every dispatch (`memory.rs`), so the declared ranges
//! cover most of what the frame's other draws read.
//!
//! What landing would give Y there is X's bytes after its flight: the copy
//! back puts into title memory the bytes the card changed, and leaves the
//! rest as title memory had them, which is what X took when its flight
//! went, as long as the title wrote none of them since. So when
//!
//! - every byte of the overlap is declared written by one other copy only
//!   (no read-back, no bottom-of-pipe write, not Y itself),
//! - X took those pages whole and the title has not written them since,
//! - Y is a plain copy (not in the card's own memory, not title memory in
//!   place) and this submission's shaders do not write it there,
//!
//! the refresh goes on for the rest, and X's bytes are copied into Y on the
//! card after X's flight and before this submission's lists: on one queue,
//! in order, exactly where the console's single memory would have them.
//! Anything else waits as before. Platform-neutral planning; the host side
//! is in `host.rs`.

/// Why a refresh was not served on the card (the frame line's counts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refuse {
    /// A flight's bottom-of-pipe write lies in the range.
    Immed = 0,
    /// A texture read-back of a flight lands in the range.
    ReadBack = 1,
    /// Two copies' flights write the same bytes.
    TwoWriters = 2,
    /// Y's own flights write the same bytes as another copy's.
    OwnWrites = 3,
    /// Y is in the card's memory or title memory in place.
    Kind = 4,
    /// The source copy did not take the pages whole, or the title wrote
    /// them since.
    TitleWrote = 5,
    /// This submission's shaders write Y there.
    Declared = 6,
    /// An earlier redirect into Y there is still in the air and this one
    /// does not cover it.
    Pending = 7,
    /// The source copy is not known among the flights.
    NoSource = 8,
}

pub(crate) const REFUSALS: usize = 9;

/// Bytes `[from, to)` of title memory, for copy `owner` (an identity: the
/// `Shadow` pointer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Piece {
    pub owner: usize,
    pub from: usize,
    pub to: usize,
}

/// The pieces of `[from, to)` refreshed for copy `copy` that other copies'
/// flights write (`by_copy`: `(owner, from, to)` joined per owner, `0` for
/// read-backs; `immed`: bottom-of-pipe writes, joined), each to be copied
/// from its owner on the card; or why that cannot be done. Empty if
/// nothing of another copy's lies there.
pub(crate) fn plan_refresh(
    copy: usize,
    from: usize,
    to: usize,
    by_copy: &[(usize, usize, usize)],
    immed: &[(usize, usize)],
) -> Result<Vec<Piece>, Refuse> {
    if from >= to {
        return Ok(Vec::new());
    }
    if immed.iter().any(|&(a, b)| a < to && b > from) {
        return Err(Refuse::Immed);
    }
    let hits: Vec<(usize, usize, usize)> = by_copy
        .iter()
        .filter(|&&(_, a, b)| a < to && b > from)
        .map(|&(o, a, b)| (o, a.max(from), b.min(to)))
        .collect();
    let mut pieces: Vec<Piece> = Vec::new();
    for &(owner, a, b) in &hits {
        if owner == copy {
            continue;
        }
        if owner == 0 {
            return Err(Refuse::ReadBack);
        }
        pieces.push(Piece { owner, from: a, to: b });
    }
    // One writer per byte: another owner's (or Y's own) range over a piece
    // makes what landing gives depend on the order of their copy-backs.
    for p in &pieces {
        for &(owner, a, b) in &hits {
            if owner != p.owner && a < p.to && b > p.from {
                return Err(if owner == copy { Refuse::OwnWrites } else { Refuse::TwoWriters });
            }
        }
    }
    pieces.sort_unstable_by_key(|p| (p.from, p.owner));
    Ok(pieces)
}

/// Whether `[from, to)` is covered by `pieces` (sorted by `from`, not
/// overlapping).
pub(crate) fn covered(pieces: &[Piece], from: usize, to: usize) -> bool {
    let mut at = from;
    for p in pieces {
        if p.to <= at {
            continue;
        }
        if p.from > at {
            return false;
        }
        at = p.to;
        if at >= to {
            return true;
        }
    }
    at >= to
}

/// Whether a copy that took pages `[first, last]` (write-watch pages)
/// still holds title memory there: each page taken whole once
/// (`copied[page - base] != 0`) and not written since (`written`: last
/// writes by page, any order).
pub(crate) fn took_current(copied: &[u64], base: usize, first: usize, last: usize, written: &[(usize, u64)]) -> bool {
    (first..=last).all(|page| {
        let Some(&at) = page.checked_sub(base).and_then(|i| copied.get(i)) else {
            return false;
        };
        at != 0 && !written.iter().any(|&(p, w)| p == page && w > at)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const Y: usize = 0x100;
    const X: usize = 0x200;
    const Z: usize = 0x300;

    #[test]
    fn nothing_of_another_copy_needs_nothing() {
        assert_eq!(plan_refresh(Y, 0x1000, 0x2000, &[(Y, 0x1000, 0x2000), (X, 0x3000, 0x4000)], &[]), Ok(vec![]));
        assert_eq!(plan_refresh(Y, 0x1000, 0x1000, &[(X, 0, 0x9000)], &[]), Ok(vec![]));
    }

    #[test]
    fn another_copys_writes_are_cut_to_the_refresh() {
        let by_copy = [(X, 0x0800, 0x1400), (X, 0x1800, 0x5000)];
        assert_eq!(
            plan_refresh(Y, 0x1000, 0x2000, &by_copy, &[]),
            Ok(vec![Piece { owner: X, from: 0x1000, to: 0x1400 }, Piece { owner: X, from: 0x1800, to: 0x2000 }])
        );
    }

    #[test]
    fn two_writers_of_one_byte_wait() {
        let by_copy = [(X, 0x1000, 0x1800), (Z, 0x1700, 0x2000)];
        assert_eq!(plan_refresh(Y, 0x1000, 0x2000, &by_copy, &[]), Err(Refuse::TwoWriters));
        // Side by side is fine.
        let by_copy = [(X, 0x1000, 0x1800), (Z, 0x1800, 0x2000)];
        assert_eq!(plan_refresh(Y, 0x1000, 0x2000, &by_copy, &[]).map(|p| p.len()), Ok(2));
    }

    #[test]
    fn own_writes_under_anothers_wait() {
        let by_copy = [(Y, 0x1000, 0x1100), (X, 0x1000, 0x2000)];
        assert_eq!(plan_refresh(Y, 0x1000, 0x2000, &by_copy, &[]), Err(Refuse::OwnWrites));
        // Own writes elsewhere in the range do not matter.
        let by_copy = [(Y, 0x1000, 0x1100), (X, 0x1800, 0x2000)];
        assert_eq!(plan_refresh(Y, 0x1000, 0x2000, &by_copy, &[]).map(|p| p.len()), Ok(1));
    }

    #[test]
    fn read_backs_and_bottom_of_pipe_writes_wait() {
        assert_eq!(plan_refresh(Y, 0x1000, 0x2000, &[(0, 0x1f00, 0x2100)], &[]), Err(Refuse::ReadBack));
        assert_eq!(plan_refresh(Y, 0x1000, 0x2000, &[], &[(0x1ff8, 0x2000)]), Err(Refuse::Immed));
        // Outside the range they do not.
        assert_eq!(plan_refresh(Y, 0x1000, 0x2000, &[(0, 0x2000, 0x2100)], &[(0x0ff8, 0x1000)]), Ok(vec![]));
    }

    #[test]
    fn coverage() {
        let p = [Piece { owner: X, from: 0x10, to: 0x20 }, Piece { owner: Z, from: 0x20, to: 0x30 }];
        assert!(covered(&p, 0x10, 0x30));
        assert!(covered(&p, 0x18, 0x28));
        assert!(!covered(&p, 0x08, 0x18));
        assert!(!covered(&p, 0x28, 0x38));
        assert!(!covered(&[], 0x10, 0x11));
        let gap = [Piece { owner: X, from: 0x10, to: 0x18 }, Piece { owner: X, from: 0x19, to: 0x30 }];
        assert!(!covered(&gap, 0x10, 0x30));
    }

    #[test]
    fn a_page_counts_only_if_taken_whole_and_not_written_since() {
        // Pages 10..=12 of a copy starting at page 10.
        let copied = [5, 7, 0];
        assert!(took_current(&copied, 10, 10, 11, &[(10, 5), (11, 3)]));
        assert!(!took_current(&copied, 10, 10, 11, &[(11, 8)]));
        // Never taken whole.
        assert!(!took_current(&copied, 10, 12, 12, &[]));
        // Outside the copy.
        assert!(!took_current(&copied, 10, 9, 10, &[]));
        assert!(!took_current(&copied, 10, 13, 13, &[]));
    }

    /// The exactness argument on bytes: what landing then refreshing gives
    /// Y equals the refresh now plus X's card bytes copied over the piece,
    /// when the title did not write the piece since X took it.
    #[test]
    fn serving_on_the_card_gives_what_landing_gives() {
        // Title memory, 16 bytes; X declared writes over [4, 12).
        let title: Vec<u8> = (0..16).collect();
        let mut x_card = title.clone(); // X took it all when its flight went
        for b in &mut x_card[6..9] {
            *b = 0xEE; // what X's shaders wrote
        }
        // Landing: the bytes the card changed go to title memory; Y then
        // refreshes all of it.
        let mut landed = title.clone();
        for i in 4..12 {
            if x_card[i] != title[i] {
                landed[i] = x_card[i];
            }
        }
        let y_after_wait = landed.clone();
        // Served on the card: Y refreshed from title memory now, then X's
        // card bytes over the piece.
        let pieces = plan_refresh(Y, 0, 16, &[(X, 4, 12)], &[]).unwrap();
        let mut y_now = title.clone();
        for p in pieces {
            y_now[p.from..p.to].copy_from_slice(&x_card[p.from..p.to]);
        }
        assert_eq!(y_now, y_after_wait);
    }
}
