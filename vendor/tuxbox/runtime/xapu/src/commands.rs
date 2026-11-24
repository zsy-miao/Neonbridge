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
/// TUXBOX_XAPU_EAGER=0 keeps the old strict answers (TOO_SMALL when the
/// packet holds more than frame_count frames, BAD_PACKET with no output,
/// always interleaved). Decoding already happens when the command is queued.
fn lenient() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_XAPU_EAGER").map_or(true, |v| v.trim() != "0"))
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

    pub(super) unsafe fn write_to(&self, out: *mut u8) -> i32 {
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
                    if !lenient() {
                        let skipped = field(32).min(frames);
                        if frames > field(36) || (frames - skipped) * channels * 4 > capacity {
                            return Err(TOO_SMALL);
                        }
                    }
                    self.pcm.resize(frames * channels, 0.0);
                    let frames = decoder
                        .decode_interleaved(compressed, &mut self.pcm)
                        .map_err(|_| BAD_PACKET)?;
                    if !lenient() {
                        let skipped = field(32).min(frames);
                        return Ok((skipped * channels, (frames - skipped) * channels * 4));
                    }
                    let (first, count) = kept(frames, field(32), field(36));
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
                        let frames = field(36).min(capacity / (channels.max(1) * 4));
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
                        completion.bytes[52..56].copy_from_slice(&(bytes as u32).to_le_bytes());
                        completion.bytes[56..64].copy_from_slice(&output.to_le_bytes());
                        completion.bytes[64..68]
                            .copy_from_slice(&(packet_bytes as u32).to_le_bytes());
                        completion.bytes[72..80].copy_from_slice(&packet.to_le_bytes());
                    }
                    Err(status) => completion.result = status,
                }
            }
            _ => return Err(INVALID),
        }
        completion.bytes[36..44].copy_from_slice(&timestamp().to_le_bytes());
        Ok(completion)
    }
}
