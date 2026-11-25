use super::{Arena, BAD_STREAM, INVALID, opus};
use std::ptr;
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

const ALREADY_ACTIVE: i32 = 0x8ac8_1005u32 as i32;
const TOO_SMALL: i32 = 0x8ac8_1006u32 as i32;
const BAD_PACKET: i32 = 0x8ac8_1008u32 as i32;
/// Connection flag: two-channel output comes one channel after the other.
const DEINTERLEAVED: u32 = 4;

/// rebox be278e9 (re-derived): a Process command keeps up to frame_count
/// frames from first_frame on, deinterleaved when the connection asks, and a
/// packet that cannot be decoded gives silence of the length asked for, not
/// an error that may make the title stop the stream.
/// Default-on (user order: fixes ship on); TUXBOX_XAPU_EAGER=0 keeps the old
/// strict answers (TOO_SMALL when the packet holds more than frame_count
/// frames, BAD_PACKET with no output, always interleaved). Decoding already
/// happens when the command is queued.
fn lenient() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_XAPU_EAGER").map_or(true, |v| v.trim() != "0"))
}

/// TUXBOX_XAPU_STRICT=1: the old strict decode (TOO_SMALL past frame_count
/// or the output, always interleaved). Default: as VIctory-fxceless apu.rs
/// decode (GPL-3.0), the parity target: frames kept from first_frame, at
/// most frame_count and what fits the output, deinterleaved when the
/// connection asks. Before, that path ran only under TUXBOX_XAPU_EAGER=1,
/// so a deinterleaved stereo stream got interleaved samples.
fn strict() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !lenient() && std::env::var("TUXBOX_XAPU_STRICT").is_ok_and(|v| v.trim() == "1"))
}

fn clamp() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_XAPU_CLAMP").map_or(true, |v| v.trim() != "0"))
}

/// Frames kept from a packet of `frames`: from `first` on, at most `count`.
fn kept(frames: usize, first: usize, count: usize) -> (usize, usize) {
    let first = first.min(frames);
    (first, (frames - first).min(count))
}

/// Interleaved stereo (L R L R) to planar (L... R...).
fn deinterleave(samples: &[f32], out: &mut Vec<f32>) {
    let frames = samples.len() / 2;
    out.clear();
    out.extend((0..frames).map(|i| samples[2 * i]));
    out.extend((0..frames).map(|i| samples[2 * i + 1]));
}

#[cfg(test)]
#[test]
fn process_keeps_frames_from_first_up_to_count_and_splits_channels() {
    assert_eq!(kept(960, 100, 1024), (100, 860));
    assert_eq!(kept(960, 0, 480), (0, 480));
    assert_eq!(kept(960, 2000, 480), (960, 0));
    let mut out = Vec::new();
    deinterleave(&[1.0, -1.0, 2.0, -2.0], &mut out);
    assert_eq!(out, [1.0, 2.0, -1.0, -2.0]);
}

#[derive(Default)]
pub(super) struct Stream {
    pub(super) pending: usize,
    pub(super) decoder: Option<opus::Decoder>,
    channels: usize,
    pcm: Vec<f32>,
}

pub(super) struct Completion {
    bytes: [u8; 80],
    result: i32,
}

impl Completion {
    pub(super) fn kind(&self) -> u32 {
        self.word(0)
    }
    /// Diagnostics: PCM bytes a decode command produced (0 for other commands).
    pub(super) fn decoded_bytes(&self) -> usize {
        if self.kind() == 3 && self.result == 0 { self.word(52) as usize } else { 0 }
    }
    pub(super) fn failed(&self) -> bool {
        self.result != 0
    }
    pub(super) fn stream(&self) -> usize {
        self.word(4) as usize
    }

    fn word(&self, offset: usize) -> u32 {
        u32::from_le_bytes(self.bytes[offset..offset + 4].try_into().unwrap())
    }

    /// The status for XApuDequeueResult: with the reference result layout
    /// it is carried in the record's error field and Dequeue answers S_OK.
    pub(super) unsafe fn write_to(&self, out: *mut u8) -> i32 {
        if ref_layout() {
            let mut bytes = self.bytes;
            bytes[44..48].copy_from_slice(&self.result.to_le_bytes());
            if !rebox_layout() {
                // VIctory-fxceless apu.rs:384 (GPL-3.0): every kind gets the
                // full 80-byte record; outside Process it is all zero past
                // the header but the error and pitch 1.0.
                if self.kind() != 3 {
                    bytes[24..44].fill(0);
                    bytes[48..80].fill(0);
                    bytes[64..68].copy_from_slice(&1.0f32.to_le_bytes());
                }
                unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), out, 80) };
                return 0;
            }
            unsafe {
                if self.kind() == 3 {
                    ptr::copy_nonoverlapping(bytes.as_ptr(), out, 80);
                } else {
                    ptr::copy_nonoverlapping(bytes.as_ptr(), out, 24);
                    ptr::copy_nonoverlapping(bytes.as_ptr().add(36), out.add(36), 12);
                }
            }
            return 0;
        }
        // Non-process completions echo the 24-byte command header. Bytes between
        // the header and the two timing fields belong to the caller.
        if self.kind() == 3 {
            unsafe { ptr::copy_nonoverlapping(self.bytes.as_ptr(), out, 80) };
        } else {
            unsafe {
                ptr::copy_nonoverlapping(self.bytes.as_ptr(), out, 24);
                ptr::copy_nonoverlapping(self.bytes.as_ptr().add(36), out.add(36), 12);
            }
        }
        self.result
    }
}

/// The Process result record as VIctory (apu.rs) and rebox (xapu.c) lay it
/// out, re-derived: state 3 (output ready, more input wanted) @24, usec 0
/// @36, status in error @44, kept frames @48, output length/address
/// @52/@56, pitch @64 (the command's target pitch), bytes consumed @68,
/// packet address @72; Dequeue answers S_OK. A failed Dequeue made the title
/// skip the result, so that stream's slot never came back (leonida
/// xapu_chad). TUXBOX_XAPU_REF_LAYOUT=0: the old record (packet bytes @64,
/// QPC usec, the status as Dequeue's answer).
pub(super) fn ref_layout() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_XAPU_REF_LAYOUT").map_or(true, |v| v.trim() != "0"))
}

/// TUXBOX_XAPU_REBOX_LAYOUT=1: rebox xapu.c's record (state 3, the command's
/// target pitch, only the header and timing words for non-Process kinds).
/// Default: VIctory-fxceless apu.rs:384, the parity target (state 0, pitch
/// 1.0, the full 80-byte record for every kind).
pub(super) fn rebox_layout() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_XAPU_REBOX_LAYOUT").is_ok_and(|v| v.trim() == "1"))
}

/// The process result fields after the header, reference layout.
fn fill_result(bytes: &mut [u8; 80], frames: u32, output_length: u32, output: usize, pitch: f32, input_length: u32, input: usize) {
    let rebox = rebox_layout();
    bytes[24..28].copy_from_slice(&(if rebox { 3u32 } else { 0 }).to_le_bytes());
    bytes[28..44].fill(0);
    bytes[48..52].copy_from_slice(&frames.to_le_bytes());
    bytes[52..56].copy_from_slice(&output_length.to_le_bytes());
    bytes[56..64].copy_from_slice(&output.to_le_bytes());
    let pitch = if rebox && pitch.is_finite() && pitch > 0.0 { pitch } else { 1.0 };
    bytes[64..68].copy_from_slice(&pitch.to_le_bytes());
    bytes[68..72].copy_from_slice(&input_length.to_le_bytes());
    bytes[72..80].copy_from_slice(&input.to_le_bytes());
}

#[cfg(test)]
#[test]
fn reference_process_result_layout() {
    let mut bytes = [0xaau8; 80];
    fill_result(&mut bytes, 960, 7680, 0x1000, f32::NAN, 120, 0x2000);
    assert_eq!(&bytes[24..28], &(if rebox_layout() { 3u32 } else { 0 }).to_le_bytes());
    assert_eq!(&bytes[36..44], &[0; 8]);
    assert_eq!(&bytes[48..52], &960u32.to_le_bytes());
    assert_eq!(&bytes[52..56], &7680u32.to_le_bytes());
    assert_eq!(&bytes[56..64], &0x1000usize.to_le_bytes());
    assert_eq!(&bytes[64..68], &1.0f32.to_le_bytes());
    assert_eq!(&bytes[68..72], &120u32.to_le_bytes());
    assert_eq!(&bytes[72..80], &0x2000usize.to_le_bytes());
}

fn timestamp() -> u64 {
    let mut ticks = 0;
    let mut frequency = 0;
    unsafe {
        QueryPerformanceCounter(&mut ticks);
        QueryPerformanceFrequency(&mut frequency);
    }
    if frequency <= 0 {
        return 0;
    }
    (ticks as u128 * 10_000_000 / frequency as u128) as u64
}

impl Stream {
    pub(super) fn channels(&self) -> usize {
        self.channels
    }
    // The caller supplies the packet of the size specified by its command code.
    pub(super) unsafe fn submit(
        &mut self,
        input: *const u8,
        arena: &Arena,
        flags: u32,
    ) -> Result<Completion, i32> {
        let kind = unsafe { input.cast::<u32>().read_unaligned() };
        let mut completion = Completion {
            bytes: [0; 80],
            result: 0,
        };
        unsafe { ptr::copy_nonoverlapping(input, completion.bytes.as_mut_ptr(), 24) };
        match kind {
            1 => {
                if self.decoder.is_some() {
                    return Err(ALREADY_ACTIVE);
                }
                let channels = unsafe { input.add(24).cast::<u32>().read_unaligned() } as usize;
                self.decoder = Some(opus::Decoder::new(channels).map_err(|_| INVALID)?);
                self.channels = channels;
            }
            2 if self.decoder.is_some() => {}
            5 if self.decoder.is_some() => {
                self.decoder
                    .as_mut()
                    .unwrap()
                    .reset()
                    .map_err(|_| INVALID)?;
            }
            2 | 5 => return Err(BAD_STREAM),
            3 => {
                let decoder = self.decoder.as_mut().ok_or(BAD_STREAM)?;
                let record = unsafe { input.cast::<[u8; 64]>().read_unaligned() };
                let field =
                    |offset| u32::from_le_bytes(record[offset..][..4].try_into().unwrap()) as usize;
                let address =
                    |offset| usize::from_le_bytes(record[offset..][..8].try_into().unwrap());
                let packet = address(44);
                let packet_bytes = field(40);
                let target_pitch = f32::from_le_bytes(record[28..32].try_into().unwrap());
                let output = address(56);
                let capacity = field(52);
                if !arena.contains(packet, packet_bytes) || !arena.contains(output, capacity) {
                    return Err(INVALID);
                }
                let compressed =
                    unsafe { std::slice::from_raw_parts(packet as *const u8, packet_bytes) };
                let channels = self.channels;
                let decoded = (|| {
                    let frames = opus::packet_samples(compressed).map_err(|_| BAD_PACKET)?;
                    if strict() {
                        let skipped = field(32).min(frames);
                        if frames > field(36) || (frames - skipped) * channels * 4 > capacity {
                            return Err(TOO_SMALL);
                        }
                    }
                    self.pcm.resize(frames * channels, 0.0);
                    let frames = decoder
                        .decode_interleaved(compressed, &mut self.pcm)
                        .map_err(|_| BAD_PACKET)?;
                    if strict() {
                        let skipped = field(32).min(frames);
                        return Ok((skipped * channels, (frames - skipped) * channels * 4));
                    }
                    // rebox xapu.c:222-230 (GPL-2.0, credit to its authors):
                    // frame_count 0 keeps every decoded frame, and what does
                    // not fit the output is clamped to capacity/(ch*4), not
                    // refused. TUXBOX_XAPU_CLAMP=0 keeps the TOO_SMALL answer.
                    let (first, count) = if clamp() {
                        let asked = if field(36) == 0 { frames } else { field(36) };
                        let (first, count) = kept(frames, field(32), asked);
                        (first, count.min(capacity / (channels.max(1) * 4)))
                    } else {
                        kept(frames, field(32), field(36))
                    };
                    if count * channels * 4 > capacity {
                        return Err(TOO_SMALL);
                    }
                    if channels == 2 && flags & DEINTERLEAVED != 0 {
                        let mut planar = Vec::new();
                        deinterleave(&self.pcm[first * 2..(first + count) * 2], &mut planar);
                        self.pcm = planar;
                        return Ok((0, count * channels * 4));
                    }
                    Ok((first * channels, count * channels * 4))
                })();
                // Not decoded: silence of the length asked for, if it fits.
                let decoded = match decoded {
                    Err(BAD_PACKET) if lenient() => {
                        // rebox xapu.c:222: frame_count 0 asks for the
                        // connection's frames_per_output, else 960; ours is
                        // always 0 (connect refuses a nonzero one), so 960.
                        let asked = if field(36) == 0 && clamp() { 960 } else { field(36) };
                        let frames = asked.min(capacity / (channels.max(1) * 4));
                        self.pcm.clear();
                        self.pcm.resize(frames * channels, 0.0);
                        static SILENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                        let n = SILENT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                        if n.is_power_of_two() {
                            unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(
                                format!("TuxBox xapu: Opus packet of {packet_bytes} bytes not decoded: {frames} frames of silence ({n} so far)\n\0").as_ptr()) };
                        }
                        Ok((0, frames * channels * 4))
                    }
                    other => other,
                };
                match decoded {
                    Ok((first_sample, bytes)) => {
                        // Decode into stream-owned PCM so overlapping guest regions are safe.
                        unsafe {
                            ptr::copy_nonoverlapping(
                                self.pcm.as_ptr().add(first_sample).cast::<u8>(),
                                output as *mut u8,
                                bytes,
                            )
                        };
                        if ref_layout() {
                            let frames = bytes / (channels.max(1) * 4);
                            fill_result(&mut completion.bytes, frames as u32, bytes as u32, output,
                                target_pitch, packet_bytes as u32, packet);
                        } else {
                            completion.bytes[52..56].copy_from_slice(&(bytes as u32).to_le_bytes());
                            completion.bytes[56..64].copy_from_slice(&output.to_le_bytes());
                            completion.bytes[64..68]
                                .copy_from_slice(&(packet_bytes as u32).to_le_bytes());
                            completion.bytes[72..80].copy_from_slice(&packet.to_le_bytes());
                        }
                    }
                    Err(status) => {
                        completion.result = status;
                        if ref_layout() {
                            fill_result(&mut completion.bytes, 0, 0, output, target_pitch, packet_bytes as u32, packet);
                        }
                    }
                }
            }
            _ => return Err(INVALID),
        }
        if !ref_layout() {
            completion.bytes[36..44].copy_from_slice(&timestamp().to_le_bytes());
        }
        Ok(completion)
    }
}
