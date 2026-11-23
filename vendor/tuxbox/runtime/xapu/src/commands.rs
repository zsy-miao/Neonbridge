use super::{Arena, BAD_STREAM, INVALID, opus};
use std::ptr;
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

const ALREADY_ACTIVE: i32 = 0x8ac8_1005u32 as i32;
const TOO_SMALL: i32 = 0x8ac8_1006u32 as i32;
const BAD_PACKET: i32 = 0x8ac8_1008u32 as i32;

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
    // The caller supplies the packet of the size specified by its command code.
    pub(super) unsafe fn submit(
        &mut self,
        input: *const u8,
        arena: &Arena,
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
                let decoded = (|| {
                    let frames = opus::packet_samples(compressed).map_err(|_| BAD_PACKET)?;
                    let skipped = field(32).min(frames);
                    let samples = frames * self.channels;
                    if frames > field(36) || (frames - skipped) * self.channels * 4 > capacity {
                        return Err(TOO_SMALL);
                    }
                    self.pcm.resize(samples, 0.0);
                    let frames = decoder
                        .decode_interleaved(compressed, &mut self.pcm)
                        .map_err(|_| BAD_PACKET)?;
                    Ok((
                        skipped * self.channels,
                        (frames - skipped) * self.channels * 4,
                    ))
                })();
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
