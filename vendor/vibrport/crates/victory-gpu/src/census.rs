//! `VICTORY_CENSUS=1`: a census of the title buffers whose host copies are in
//! the card's memory (`crate::shadow::cards`), written to the log every
//! `VICTORY_CENSUS_SECONDS` (10). Logging only: nothing here changes what
//! is copied, when, or where.
//!
//! Per copy: the title range, the `XMemVirtualAlloc` reservation it lies in
//! with the flags the title asked for (the runtime drops write-combining,
//! uncached and the GPU access bits; they are kept here), how much of the
//! range is physical-page views and how much of that shows a page at more
//! than one address, and per frame the bytes sent up to the card and the
//! bytes fetched back from it. A copy the title only writes (sent up,
//! nothing fetched), in memory it asked write-combining, at one address, is
//! the kind that could live in the card's memory with the CPU writing it
//! through the BAR instead of a copy.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Whether the census is on (`VICTORY_CENSUS=1`).
pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_CENSUS").is_some_and(|v| v == "1"))
}

fn period() -> Duration {
    static S: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    Duration::from_secs(*S.get_or_init(|| {
        std::env::var("VICTORY_CENSUS_SECONDS").ok().and_then(|v| v.trim().parse().ok()).filter(|&s| s > 0).unwrap_or(10)
    }))
}

/// A reservation of `XMemVirtualAlloc`, as the title asked for it.
struct Reservation {
    len: usize,
    kind: u32,
    flags: u64,
    protect: u32,
}

static RESERVATIONS: Mutex<BTreeMap<usize, Reservation>> = Mutex::new(BTreeMap::new());

/// The runtime reserved `[at, at + len)` for the title (`XMemVirtualAlloc`
/// at an address of its choice), with the title's own `kind`, XMem `flags`
/// and console `protect` (before the runtime cleans them).
pub fn title_memory_reserved(at: usize, len: usize, kind: u32, flags: u64, protect: u32) {
    if !(on() || skip_wc_back()) || at == 0 || len == 0 {
        return;
    }
    RESERVATIONS.lock().unwrap_or_else(|e| e.into_inner()).insert(at, Reservation { len, kind, flags, protect });
}

/// `VICTORY_SKIP_WC_BACK=1`, an EXPERIMENT, not a fix: what shaders wrote
/// to a card copy of title memory the title reserved write-combining is not
/// fetched back into title memory. It tests whether the title's CPU reads
/// that memory at all (the census found 97 of 100 MB a frame fetched back
/// from it). Expect artifacts where the title's CPU writes part of a
/// 64 KiB piece the card wrote: the next upload sends the stale bytes there.
pub(crate) fn skip_wc_back() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = skip_wc_compare() || std::env::var_os("VICTORY_SKIP_WC_BACK").is_some_and(|v| v == "1");
        if on {
            tracing::warn!("VICTORY_SKIP_WC_BACK=1: shader writes to write-combining title memory are NOT copied back (experiment)");
        }
        on
    })
}

/// `VICTORY_SKIP_WC_COMPARE=1`, the second half of the same EXPERIMENT
/// (it implies [`skip_wc_back`]): the card does not hash or compare the
/// declared writes of copies of write-combining title memory at all, since
/// nothing of them is fetched back. The CPU's changes are still sent up.
pub(crate) fn skip_wc_compare() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_SKIP_WC_COMPARE").is_some_and(|v| v == "1");
        if on {
            tracing::warn!("VICTORY_SKIP_WC_COMPARE=1: copies of write-combining title memory are not hashed or compared on the card (experiment)");
        }
        on
    })
}

/// Whether the title buffer at `start` lies in memory the title reserved
/// write-combining (needs the census or a skip switch on, which record the
/// reservations).
pub(crate) fn is_wc(start: u64) -> bool {
    RESERVATIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .range(..=start as usize)
        .next_back()
        .is_some_and(|(base, r)| *base + r.len > start as usize && r.protect & 0x400 != 0)
}

/// Declared ranges left out of the compare by [`skip_wc_compare`].
pub(crate) static UNCOMPARED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Pieces left out by [`skip_wc_back`]: count and bytes.
pub(crate) static SKIPPED: [std::sync::atomic::AtomicU64; 2] =
    [std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0)];

/// With [`skip_wc_back`]: whether a piece of `bytes` of the copy of the
/// title buffer at `start` is left on the card (counted).
pub(crate) fn skip_back(start: u64, bytes: u64) -> bool {
    if !skip_wc_back() {
        return false;
    }
    let wc = is_wc(start);
    if wc {
        use std::sync::atomic::Ordering;
        let n = SKIPPED[0].fetch_add(1, Ordering::Relaxed) + 1;
        let b = SKIPPED[1].fetch_add(bytes, Ordering::Relaxed) + bytes;
        if n.is_power_of_two() {
            tracing::info!("VICTORY_SKIP_WC_BACK: {n} pieces ({} MB) of write-combining memory left on the card so far", b >> 20);
        }
    }
    wc
}

/// Host resources made, by where: count and bytes ([`made`]).
const KINDS: [&str; 9] = ["read-back", "upload", "shared", "uav", "texture", "reserved texture", "depth twin", "placed in title memory", "host list"];
static MADE: [std::sync::atomic::AtomicU64; 18] = [const { std::sync::atomic::AtomicU64::new(0) }; 18];
static MADE_LAST: Mutex<[u64; 18]> = Mutex::new([0; 18]);

/// With the census on: a host resource of kind `kind` ([`KINDS`]) and
/// `bytes` (0 when not known) was asked of the device.
pub(crate) fn made(kind: usize, bytes: u64) {
    if on() && kind < KINDS.len() {
        use std::sync::atomic::Ordering;
        MADE[kind * 2].fetch_add(1, Ordering::Relaxed);
        MADE[kind * 2 + 1].fetch_add(bytes, Ordering::Relaxed);
    }
}

/// Counters of one copy, by the title address it starts at.
#[derive(Default, Clone, Copy)]
struct Copy {
    len: u64,
    made_frame: u64,
    sent: u64,
    sent_ranges: u64,
    compared: u64,
    fetched: u64,
    fetched_pieces: u64,
}

struct Census {
    copies: BTreeMap<u64, Copy>,
    /// The counters at the last report, to print per-frame rates.
    last: BTreeMap<u64, Copy>,
    last_at: Option<(Instant, u64)>,
}

static CENSUS: Mutex<Census> = Mutex::new(Census { copies: BTreeMap::new(), last: BTreeMap::new(), last_at: None });

fn census() -> std::sync::MutexGuard<'static, Census> {
    CENSUS.lock().unwrap_or_else(|e| e.into_inner())
}

/// A copy of the title buffer at `start` of `len` bytes was made in the
/// card's memory.
pub(crate) fn copy_made(start: u64, len: u64) {
    if !on() {
        return;
    }
    let frame = crate::frame::current();
    let mut c = census();
    let e = c.copies.entry(start).or_default();
    e.len = len;
    e.made_frame = frame;
}

/// `bytes` of the copy at `start` were sent up to the card in `ranges` ranges.
pub(crate) fn sent(start: u64, bytes: u64, ranges: u64) {
    if !on() {
        return;
    }
    {
        let mut c = census();
        let e = c.copies.entry(start).or_default();
        e.sent += bytes;
        e.sent_ranges += ranges;
    }
    report_if_due();
}

/// `pieces` 64 KiB pieces of the copy at `start` were compared on the card.
pub(crate) fn compared(start: u64, pieces: u64) {
    if on() {
        census().copies.entry(start).or_default().compared += pieces;
    }
}

/// `bytes` of the copy at `start` were fetched back from the card, in one piece.
pub(crate) fn fetched(start: u64, bytes: u64) {
    if !on() {
        return;
    }
    {
        let mut c = census();
        let e = c.copies.entry(start).or_default();
        e.fetched += bytes;
        e.fetched_pieces += 1;
    }
    report_if_due();
}

/// The console protection bits the runtime drops, spelled out.
fn protect_words(p: u32) -> String {
    let mut w = Vec::new();
    if p & 0x400 != 0 {
        w.push("WC");
    }
    if p & 0x200 != 0 {
        w.push("uncached");
    }
    if p & 0x100 != 0 {
        w.push("guard");
    }
    let gpu = p & 0x7_f800;
    let mut s = w.join("+");
    if gpu != 0 {
        if !s.is_empty() {
            s.push('+');
        }
        s.push_str(&format!("gpu{gpu:#x}"));
    }
    if s.is_empty() { "-".to_owned() } else { s }
}

fn report_if_due() {
    let frame = crate::frame::current();
    let now = Instant::now();
    let mut c = match CENSUS.try_lock() {
        Ok(c) => c,
        Err(_) => return,
    };
    let Some((then, then_frame)) = c.last_at else {
        c.last_at = Some((now, frame));
        return;
    };
    if now.duration_since(then) < period() {
        return;
    }
    let frames = frame.saturating_sub(then_frame).max(1) as f64;
    let secs = now.duration_since(then).as_secs_f64();
    c.last_at = Some((now, frame));
    let copies = c.copies.clone();
    let last = std::mem::replace(&mut c.last, copies.clone());
    drop(c);

    let reservations = RESERVATIONS.lock().unwrap_or_else(|e| e.into_inner());
    let mb = |b: u64| b as f64 / 1e6;
    struct Row {
        start: u64,
        len: u64,
        sent: f64,
        ranges: f64,
        compared: f64,
        fetched: f64,
        pieces: f64,
        res: String,
        words: String,
        wc: bool,
        in_views: usize,
        multi: usize,
    }
    let mut rows: Vec<Row> = copies
        .iter()
        .map(|(&start, now_)| {
            let was = last.get(&start).copied().unwrap_or_default();
            let res = reservations
                .range(..=start as usize)
                .next_back()
                .filter(|(base, r)| *base + r.len > start as usize);
            let (res_s, words, wc) = match res {
                Some((base, r)) => (
                    format!("{base:#x}+{}MB kind {:#x} flags {:#x} protect {:#x}", r.len >> 20, r.kind, r.flags, r.protect),
                    protect_words(r.protect),
                    r.protect & 0x400 != 0,
                ),
                None => ("not in a recorded reservation".to_owned(), "?".to_owned(), false),
            };
            let (in_views, multi) = crate::inplace::view_census(start as usize, now_.len as usize);
            Row {
                start,
                len: now_.len,
                sent: mb(now_.sent - was.sent) / frames,
                ranges: (now_.sent_ranges - was.sent_ranges) as f64 / frames,
                compared: (now_.compared - was.compared) as f64 / frames,
                fetched: mb(now_.fetched - was.fetched) / frames,
                pieces: (now_.fetched_pieces - was.fetched_pieces) as f64 / frames,
                res: res_s,
                words,
                wc,
                in_views,
                multi,
            }
        })
        .collect();
    drop(reservations);
    rows.sort_by(|a, b| (b.fetched + b.sent).total_cmp(&(a.fetched + a.sent)));

    let total = |f: &dyn Fn(&Row) -> f64| rows.iter().map(f).sum::<f64>();
    let only_written = rows.iter().filter(|r| r.sent > 0.0 && r.fetched == 0.0).count();
    let fetched_wc = total(&|r| if r.wc { r.fetched } else { 0.0 });
    let fetched_multi = total(&|r| if r.multi > 0 { r.fetched } else { 0.0 });
    tracing::info!(
        target: "census",
        "census over {secs:.1} s, {frames:.0} frames: {} copies on the card, {:.0} MB; per frame {:.1} MB sent up, {:.1} MB fetched back \
         ({:.1} MB of it from write-combining memory, {:.1} MB from copies with multi-address pages), {:.0} pieces compared; \
         {only_written} copies only sent up (CPU writes, nothing fetched); {} copies with multi-address pages",
        rows.len(),
        mb(rows.iter().map(|r| r.len).sum()),
        total(&|r| r.sent),
        total(&|r| r.fetched),
        fetched_wc,
        fetched_multi,
        total(&|r| r.compared),
        rows.iter().filter(|r| r.multi > 0).count(),
    );
    {
        use std::sync::atomic::Ordering;
        let mut last = MADE_LAST.lock().unwrap_or_else(|e| e.into_inner());
        let now_made: [u64; 18] = std::array::from_fn(|i| MADE[i].load(Ordering::Relaxed));
        let parts: Vec<String> = KINDS
            .iter()
            .enumerate()
            .map(|(k, name)| {
                let n = (now_made[k * 2] - last[k * 2]) as f64 / frames;
                let b = (now_made[k * 2 + 1] - last[k * 2 + 1]) as f64 / frames / 1e6;
                format!("{name} {n:.1} ({b:.1} MB)")
            })
            .collect();
        let total: u64 = (0..KINDS.len()).map(|k| now_made[k * 2] - last[k * 2]).sum();
        *last = now_made;
        tracing::info!(
            target: "census",
            "census resources made per frame: {:.1} in all; {}",
            total as f64 / frames,
            parts.join(", ")
        );
    }
    for r in rows.iter().take(48) {
        tracing::info!(
            target: "census",
            "census copy {:#x}+{:.1}MB: per frame sent {:.2} MB in {:.1} ranges, compared {:.0} pieces, fetched {:.2} MB in {:.1} pieces; \
             views {:.1} MB, multi-address {:.1} MB; {} [{}]",
            r.start,
            mb(r.len),
            r.sent,
            r.ranges,
            r.compared,
            r.fetched,
            r.pieces,
            mb(r.in_views as u64),
            mb(r.multi as u64),
            r.words,
            r.res
        );
    }
}
