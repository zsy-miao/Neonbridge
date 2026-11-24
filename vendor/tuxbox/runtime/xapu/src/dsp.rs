//! Series CFPU2 clients and their convolution/FFT streams.
use super::{INVALID, POINTER};
use std::{
    collections::BTreeMap,
    ffi::c_void,
    ops::Range,
    sync::{
        LazyLock, Mutex, MutexGuard, Once,
        atomic::{AtomicU32, Ordering},
    },
};
use windows_sys::Win32::System::{
    Diagnostics::Debug::{OutputDebugStringA, ReadProcessMemory},
    Threading::GetCurrentProcess,
};

const NOT_IMPLEMENTED: i32 = 0x8000_4001u32 as i32;
const NO_STREAM: i32 = 0x8ac8_10a2u32 as i32;
const CLIENT_LIMIT: i32 = 0x8ac8_10a5u32 as i32;
const IR_BUDGET: i32 = 0x8ac8_10a6u32 as i32;
const IR_LENGTH: i32 = 0x8ac8_10a7u32 as i32;

struct Client {
    workspace: Range<usize>,
    stream_limit: usize,
    ir_seconds: u32,
    max_channels: u32,
    streams: BTreeMap<usize, Box<Stream>>,
}
static CLIENTS: LazyLock<Mutex<BTreeMap<usize, Box<Client>>>> = LazyLock::new(Default::default);
fn clients() -> MutexGuard<'static, BTreeMap<usize, Box<Client>>> {
    CLIENTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn XDspConnectWithMaximumStreamLimit(
    base: *mut c_void,
    bytes: u32,
    ir_seconds: u32,
    stream_limit: u32,
    max_channels: u32,
    out: *mut usize,
) -> i32 {
    if out.is_null() {
        return POINTER;
    }
    unsafe { out.write_unaligned(0) };
    let Some(end) = base.addr().checked_add(bytes as usize) else {
        return INVALID;
    };
    if base.is_null()
        || !(65536..=0x1f400000).contains(&bytes)
        || ir_seconds > 128
        || stream_limit > 256
        || max_channels > 2
    {
        return INVALID;
    }
    let mut clients = clients();
    if clients
        .values()
        .map(|client| client.ir_seconds)
        .sum::<u32>()
        + ir_seconds
        > 128
    {
        return IR_BUDGET;
    }
    if clients.len() >= 8 {
        return CLIENT_LIMIT;
    }
    let mut client = Box::new(Client {
        workspace: base.addr()..end,
        ir_seconds,
        max_channels,
        stream_limit: stream_limit as usize,
        streams: Default::default(),
    });
    let handle = (&raw mut *client).addr();
    clients.insert(handle, client);
    unsafe {
        out.write_unaligned(handle);
        OutputDebugStringA(
            c"TuxBox chad: registered convolution working memory\n"
                .as_ptr()
                .cast(),
        );
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn XDspConnect(
    base: *mut c_void,
    bytes: u32,
    ir_seconds: u32,
    out: *mut usize,
) -> i32 {
    unsafe { XDspConnectWithMaximumStreamLimit(base, bytes, ir_seconds, 256, 2, out) }
}
#[unsafe(no_mangle)]
pub extern "system" fn XDspDisconnect(client: usize) -> i32 {
    if clients().remove(&client).is_some() {
        0
    } else {
        INVALID
    }
}

// The title polls completed and result directly. Publish the written PCM before
// releasing completed, and retain this address until the stream is deactivated.
#[repr(C)]
#[derive(Default)]
struct Status {
    submitted: AtomicU32,
    completed: AtomicU32,
    first_failed: AtomicU32,
    result: AtomicU32,
}

struct Stream {
    status: Status,
    kind: u32,
    flags: u32,
    block: usize,
    channels: usize,
    zero_filter: bool,
}

fn silent_spectrum(at: usize, floats: usize) -> Result<bool, i32> {
    if at == 0 || at.checked_add(floats * 4).is_none() {
        return Err(INVALID);
    }
    // IRs can reside outside the registered I/O arena. The native provider
    // reads and copies them immediately, then permits the guest to release them.
    let mut buffer = [0.0f32; 2048];
    let mut silent = true;
    for offset in (0..floats).step_by(buffer.len()) {
        let count = buffer.len().min(floats - offset);
        let mut read = 0;
        if unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                (at + offset * 4) as _,
                buffer.as_mut_ptr().cast(),
                count * 4,
                &mut read,
            )
        } == 0
            || read != count * 4
        {
            return Err(INVALID);
        }
        for sample in &buffer[..count] {
            if !sample.is_finite() {
                return Err(NOT_IMPLEMENTED);
            }
            silent &= *sample == 0.0;
        }
    }
    Ok(silent)
}

impl Stream {
    fn open(client: &Client, words: [u32; 7]) -> Result<Self, i32> {
        let [kind, flags, n, channels, ir_len, lo, hi] = words;
        if !(1..=3).contains(&kind)
            || !matches!(channels, 1 | 2)
            || channels > client.max_channels
            || !matches!(n, 512 | 1024 | 2048)
            || (kind == 1 && n == 2048)
            || (kind != 1 && channels != 1)
            || (flags & (2 | 8) != 0 && channels != 2)
            || (flags & 4 != 0 && kind == 1)
        {
            return Err(INVALID);
        }
        let zero_filter = if kind == 1 {
            let partitions = ir_len / 2 / n;
            // Native IR data has N complex coefficients in each partition. A
            // separate second bank is present only when option bit 3 is set.
            if !(9..=2048).contains(&partitions) {
                return Err(IR_LENGTH);
            }
            let banks = if flags & 8 != 0 { 2 } else { 1 };
            let at = lo as usize | (hi as usize) << 32;
            silent_spectrum(at, (partitions * 2 * n * banks) as usize)?
        } else {
            false
        };
        Ok(Self {
            status: Status::default(),
            kind,
            flags,
            block: n as usize,
            channels: channels as usize,
            zero_filter,
        })
    }

    unsafe fn process(
        &self,
        workspace: &Range<usize>,
        command: &Command,
        envelope: usize,
        seq: *mut u32,
    ) -> i32 {
        // The observed mono submit uses two finite unit gains, base-buffer I/O
        // and no envelope. Frequency coefficient packing for non-silent FFTs
        // and nonzero convolution responses still needs a console control.
        // A transform not computed still completes at once with a silent
        // output (reverb returns dry): the title reads the completed ticket
        // one block later and treats a late one as an overrun (VIctory dsp.rs,
        // re-derived). Opt-in: TUXBOX_CHAD_STRICT=0; refused by default.
        let supported = self.channels == 1
            && self.flags == 0
            && envelope == 0
            && command.aux == 0
            && command.gains.iter().all(|v| *v == 1.0);
        let words = self.block * self.channels.max(1);
        let inside = |p: usize, n: usize| {
            p.is_multiple_of(4)
                && p >= workspace.start
                && p.checked_add(n * 4).is_some_and(|end| end <= workspace.end)
        };
        if supported && ![command.input, command.output].into_iter().all(|p| inside(p, self.block)) {
            return INVALID;
        }
        let silent_input = supported && {
            let input = unsafe { std::slice::from_raw_parts(command.input as *const f32, self.block) };
            input.iter().all(|&v| v.is_finite() && (v == 0.0 || self.zero_filter))
        };
        if !silent_input {
            if strict() {
                if supported {
                    static FIRST: Once = Once::new();
                    FIRST.call_once(|| {
                        let msg = format!("TuxBox chad: non-silent CFPU2 transform type {} block {} requires coefficient layout\n\0",self.kind,self.block);
                        unsafe { OutputDebugStringA(msg.as_ptr()); }
                    });
                }
                return NOT_IMPLEMENTED;
            }
            super::stats::dsp::SHORTCUT.fetch_add(1, Ordering::Relaxed);
            // Zero the output where it lies inside the workspace; complete either way.
            let n = if inside(command.output, words) { words } else if inside(command.output, self.block) { self.block } else { 0 };
            unsafe { (command.output as *mut f32).write_bytes(0, n) };
            return unsafe { self.complete(seq) };
        }
        unsafe { (command.output as *mut f32).write_bytes(0, self.block) };
        unsafe { self.complete(seq) }
    }

    /// Advance the completed ticket and hand it back.
    unsafe fn complete(&self, seq: *mut u32) -> i32 {
        let sequence = self
            .status
            .submitted
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        self.status.result.store(0, Ordering::Relaxed);
        self.status.completed.store(sequence, Ordering::Release);
        if !seq.is_null() {
            unsafe {
                seq.write_unaligned(sequence);
            }
        }
        0
    }
}

/// Transforms not computed are refused (no ticket advance), as before
/// 8d0b7c6: audio starved in the runs that had the silent completion, so it
/// is opt-in until a run proves it (TUXBOX_CHAD_STRICT=0 completes them with
/// a silent output).
fn strict() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| strict_from(std::env::var("TUXBOX_CHAD_STRICT").ok().as_deref()))
}

fn strict_from(value: Option<&str>) -> bool {
    value.is_none_or(|v| v.trim() != "0")
}

#[repr(C)]
pub(crate) struct Command {
    gains: [f32; 2],
    input: usize,
    output: usize,
    aux: usize,
}
const _: () = assert!(size_of::<Command>() == 32 && size_of::<Status>() == 16);

#[unsafe(no_mangle)]
pub unsafe extern "system" fn XDspActivate(
    handle: usize,
    format: *const [u32; 7],
    status: *mut usize,
    stream: *mut usize,
) -> i32 {
    if status.is_null() || stream.is_null() {
        return POINTER;
    }
    unsafe {
        stream.write_unaligned(0);
        status.write_unaligned(0);
    }
    if format.is_null() {
        return INVALID;
    }
    let mut clients = clients();
    let Some(client) = clients.get_mut(&handle) else {
        return INVALID;
    };
    if client.streams.len() >= client.stream_limit {
        return NO_STREAM;
    }
    let mut voice = match Stream::open(client, unsafe { format.read_unaligned() }) {
        Ok(voice) => Box::new(voice),
        Err(e) => return e,
    };
    let id = (&raw mut *voice).addr();
    let address = (&raw const voice.status).addr();
    client.streams.insert(id, voice);
    super::stats::dsp::ACTIVATES.fetch_add(1, Ordering::Relaxed);
    unsafe {
        status.write_unaligned(address);
        stream.write_unaligned(id);
    }
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn XDspDeactivate(stream: usize) -> i32 {
    if clients()
        .values_mut()
        .any(|c| c.streams.remove(&stream).is_some())
    {
        super::stats::dsp::DEACTIVATES.fetch_add(1, Ordering::Relaxed);
        0
    } else {
        INVALID
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "system" fn XDspSubmitCommand(
    stream: usize,
    command: *const Command,
    sequence: *mut u32,
) -> i32 {
    unsafe { XDspSubmitCommandWithEnvelope(stream, command, 0, sequence) }
}
#[unsafe(no_mangle)]
pub unsafe extern "system" fn XDspSubmitCommandWithEnvelope(
    stream: usize,
    command: *const Command,
    envelope: usize,
    sequence: *mut u32,
) -> i32 {
    if command.is_null() {
        return INVALID;
    }
    let registry = clients();
    let Some((client, voice)) = registry
        .values()
        .find_map(|c| c.streams.get(&stream).map(|v| (c, v)))
    else {
        return INVALID;
    };
    let status = unsafe {
        voice.process(
            &client.workspace,
            &command.read_unaligned(),
            envelope,
            sequence,
        )
    };
    use super::stats::dsp;
    match status {
        0 => dsp::SUBMITTED.fetch_add(1, Ordering::Relaxed),
        s if s == NOT_IMPLEMENTED => dsp::REFUSED.fetch_add(1, Ordering::Relaxed),
        _ => dsp::INVALID.fetch_add(1, Ordering::Relaxed),
    };
    dsp::note();
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_is_the_default_and_zero_opts_into_silent_completion() {
        assert!(strict_from(None));
        assert!(strict_from(Some("1")));
        assert!(!strict_from(Some("0")));
    }

    #[test]
    fn a_transform_not_computed_completes_with_silence() {
        if strict() {
            return; // only meaningful with TUXBOX_CHAD_STRICT=0
        }
        let stream = Stream { status: Status::default(), kind: 2, flags: 0, block: 4, channels: 1, zero_filter: false };
        let mut buffer = [1.0f32, 2.0, 3.0, 4.0, 9.0, 9.0, 9.0, 9.0];
        let start = buffer.as_mut_ptr().addr();
        let workspace = start..start + size_of_val(&buffer);
        let command = Command { gains: [1.0, 1.0], input: start, output: start + 16, aux: 0 };
        let mut seq = 0u32;
        let status = unsafe { stream.process(&workspace, &command, 0, &mut seq) };
        assert_eq!(status, 0);
        assert_eq!(seq, 1);
        assert_eq!(stream.status.completed.load(Ordering::Acquire), 1);
        assert_eq!(&buffer[4..], &[0.0; 4]);
        assert_eq!(&buffer[..4], &[1.0, 2.0, 3.0, 4.0]);
    }
}
