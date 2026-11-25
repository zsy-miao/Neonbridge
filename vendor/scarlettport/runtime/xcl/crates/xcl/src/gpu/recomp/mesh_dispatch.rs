//! Foundation for dispatching logical wave64 mesh programs on fixed-wave32 hardware.
//!
//! This module deliberately does not enable real programs. It defines the external
//! scratch contract and a rendezvous shape that later instruction lowerings can use.

pub const GROUP_THREADS: u32 = 128;
pub const LOGICAL_WAVE_LANES: u32 = 64;
pub const LOGICAL_WAVES: u32 = 2;
pub const HALF_LANES: u32 = 32;
pub const SCALAR_REGISTERS: u32 = 128;
const STATE_HEADER_WORDS: u32 = 7;
const STATE_WORDS: u32 = STATE_HEADER_WORDS + SCALAR_REGISTERS;
const MAILBOX_WORDS: u32 = GROUP_THREADS;
const RESULT_WORDS: u32 = GROUP_THREADS;
const RECORD_WORDS: u32 = STATE_WORDS * LOGICAL_WAVES + MAILBOX_WORDS + RESULT_WORDS;
pub const MIN_RECORD_BYTES: u32 = RECORD_WORDS * 4;
pub const RECORD_ALIGNMENT: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogicalWaveState {
    pub pc: u32,
    pub exec: [u32; 2],
    pub vcc: [u32; 2],
    pub scc: u32,
    pub epoch: u32,
    pub scalar: [u32; SCALAR_REGISTERS as usize],
}

impl Default for LogicalWaveState {
    fn default() -> Self {
        Self {
            pc: 0,
            exec: [0; 2],
            vcc: [0; 2],
            scc: 0,
            epoch: 0,
            scalar: [0; SCALAR_REGISTERS as usize],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScratchLayout {
    pub base_bytes: u32,
    pub record_stride_bytes: u32,
    pub record_count: u32,
    pub total_bytes: u32,
}

impl ScratchLayout {
    pub fn new(
        base_bytes: u32,
        record_stride_bytes: u32,
        record_count: u32,
    ) -> Result<Self, &'static str> {
        if base_bytes % RECORD_ALIGNMENT != 0 || record_stride_bytes % RECORD_ALIGNMENT != 0 {
            return Err("dispatcher scratch is not record-aligned");
        }
        if record_stride_bytes < MIN_RECORD_BYTES {
            return Err("dispatcher scratch stride is too small");
        }
        let records_bytes = record_stride_bytes
            .checked_mul(record_count)
            .ok_or("dispatcher scratch size overflow")?;
        let total_bytes = base_bytes
            .checked_add(records_bytes)
            .ok_or("dispatcher scratch address overflow")?;
        Ok(Self {
            base_bytes,
            record_stride_bytes,
            record_count,
            total_bytes,
        })
    }

    pub fn record_base(&self, record: u32) -> Result<u32, &'static str> {
        if record >= self.record_count {
            return Err("dispatcher scratch record is out of bounds");
        }
        self.base_bytes
            .checked_add(
                self.record_stride_bytes
                    .checked_mul(record)
                    .ok_or("dispatcher scratch address overflow")?,
            )
            .ok_or("dispatcher scratch address overflow")
    }

    pub fn state_offset(&self, record: u32, logical_wave: u32) -> Result<u32, &'static str> {
        if logical_wave >= LOGICAL_WAVES {
            return Err("logical wave is out of bounds");
        }
        self.record_base(record)?
            .checked_add(logical_wave * STATE_WORDS * 4)
            .ok_or("dispatcher scratch address overflow")
    }

    pub fn mailbox_offset(&self, record: u32, thread: u32) -> Result<u32, &'static str> {
        if thread >= GROUP_THREADS {
            return Err("dispatcher thread is out of bounds");
        }
        self.record_base(record)?
            .checked_add((STATE_WORDS * LOGICAL_WAVES + thread) * 4)
            .ok_or("dispatcher scratch address overflow")
    }

    pub fn result_offset(&self, record: u32, thread: u32) -> Result<u32, &'static str> {
        if thread >= GROUP_THREADS {
            return Err("dispatcher thread is out of bounds");
        }
        self.record_base(record)?
            .checked_add((STATE_WORDS * LOGICAL_WAVES + MAILBOX_WORDS + thread) * 4)
            .ok_or("dispatcher scratch address overflow")
    }
}

/// A small, deterministic model of the rendezvous contract. Both logical waves
/// always execute all three epochs; inactive waves only stop mutating state.
pub fn model_fixed_wave32(
    input: &[u32; GROUP_THREADS as usize],
) -> ([u32; GROUP_THREADS as usize], [LogicalWaveState; 2]) {
    let mut values = *input;
    let mut mailbox = [0u32; GROUP_THREADS as usize];
    let mut states = [LogicalWaveState {
        exec: [u32::MAX; 2],
        ..Default::default()
    }; 2];
    for epoch in 0..3u32 {
        mailbox.copy_from_slice(&values);
        for thread in 0..GROUP_THREADS as usize {
            let wave = thread / LOGICAL_WAVE_LANES as usize;
            let active_epochs = wave as u32 + 2;
            if epoch < active_epochs {
                let partner = thread ^ HALF_LANES as usize;
                values[thread] = values[thread]
                    .wrapping_add(mailbox[partner])
                    .wrapping_add(epoch);
            }
        }
        for (wave, state) in states.iter_mut().enumerate() {
            if epoch < wave as u32 + 2 {
                state.pc = if wave == 0 { 0x10 } else { 0x20 };
                state.vcc = [0xaaaa_aaaa ^ epoch, 0x5555_5555 ^ epoch];
                state.scc = wave as u32;
                state.epoch = epoch + 1;
                state.scalar[0] = values[wave * LOGICAL_WAVE_LANES as usize];
            }
        }
    }
    (values, states)
}

/// Direct logical-wave64 reference for the synthetic program used to validate
/// paired-half exchange and independent control state.
pub fn model_native_wave64(
    input: &[u32; GROUP_THREADS as usize],
) -> ([u32; GROUP_THREADS as usize], [LogicalWaveState; 2]) {
    let mut output = *input;
    let mut states = [LogicalWaveState {
        exec: [u32::MAX; 2],
        ..Default::default()
    }; 2];
    for wave in 0..LOGICAL_WAVES as usize {
        let begin = wave * LOGICAL_WAVE_LANES as usize;
        let end = begin + LOGICAL_WAVE_LANES as usize;
        for epoch in 0..wave as u32 + 2 {
            let previous = output[begin..end].to_vec();
            for lane in 0..LOGICAL_WAVE_LANES as usize {
                output[begin + lane] = output[begin + lane]
                    .wrapping_add(previous[lane ^ HALF_LANES as usize])
                    .wrapping_add(epoch);
            }
            states[wave].pc = if wave == 0 { 0x10 } else { 0x20 };
            states[wave].vcc = [0xaaaa_aaaa ^ epoch, 0x5555_5555 ^ epoch];
            states[wave].scc = wave as u32;
            states[wave].epoch = epoch + 1;
            states[wave].scalar[0] = output[begin];
        }
    }
    (output, states)
}

/// Standalone shader proving the dispatcher contract is expressible in HLSL.
/// Scratch is an external UAV and therefore consumes none of `xcl_lds`.
pub fn validation_hlsl() -> String {
    format!(
        r#"RWStructuredBuffer<uint> io : register(u0);
RWByteAddressBuffer scratch : register(u2);
[numthreads({GROUP_THREADS}, 1, 1)]
void main(uint gi : SV_GroupIndex) {{
    uint record_base = io[0];
    uint wave = gi / {LOGICAL_WAVE_LANES}u;
    uint lane = gi % {LOGICAL_WAVE_LANES}u;
    uint state_base = record_base + wave * {state_bytes}u;
    uint mailbox_base = record_base + {mailbox_base}u;
    uint result_base = record_base + {result_base}u;
    uint value = gi + 1u;
    if (lane == 0u) {{
        scratch.Store4(state_base, uint4(0u, 0xffffffffu, 0xffffffffu, 0u));
        scratch.Store4(state_base + 16u, uint4(0u, 0u, 0u, 0u));
    }}
    DeviceMemoryBarrierWithGroupSync();
    [unroll] for (uint epoch = 0u; epoch < 3u; ++epoch) {{
        scratch.Store(mailbox_base + gi * 4u, value);
        DeviceMemoryBarrierWithGroupSync();
        uint paired = scratch.Load(mailbox_base + (wave * {LOGICAL_WAVE_LANES}u + (lane ^ {HALF_LANES}u)) * 4u);
        if (epoch < wave + 2u) value += paired + epoch;
        if (lane == 0u && epoch < wave + 2u) {{
            scratch.Store(state_base, wave == 0u ? 0x10u : 0x20u);
            scratch.Store(state_base + 12u, 0xaaaaaaaau ^ epoch);
            scratch.Store(state_base + 16u, 0x55555555u ^ epoch);
            scratch.Store(state_base + 20u, wave);
            scratch.Store(state_base + 24u, epoch + 1u);
            scratch.Store(state_base + {state_header_bytes}u, value);
        }}
        DeviceMemoryBarrierWithGroupSync();
    }}
    scratch.Store(result_base + gi * 4u, value);
}}
"#,
        state_bytes = STATE_WORDS * 4,
        state_header_bytes = STATE_HEADER_WORDS * 4,
        mailbox_base = STATE_WORDS * LOGICAL_WAVES * 4,
        result_base = (STATE_WORDS * LOGICAL_WAVES + MAILBOX_WORDS) * 4
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_external_aligned_and_bounded() {
        let stride = MIN_RECORD_BYTES.next_multiple_of(RECORD_ALIGNMENT);
        let layout = ScratchLayout::new(512, stride, 3).unwrap();
        assert_eq!(layout.total_bytes, 512 + stride * 3);
        assert_eq!(
            layout.state_offset(2, 1).unwrap(),
            512 + stride * 2 + STATE_WORDS * 4
        );
        assert!(layout.mailbox_offset(0, GROUP_THREADS).is_err());
        assert!(layout.result_offset(3, 0).is_err());
        assert!(ScratchLayout::new(1, stride, 1).is_err());
        assert!(ScratchLayout::new(0, MIN_RECORD_BYTES - 4, 1).is_err());
        assert!(ScratchLayout::new(0, u32::MAX - 255, 2).is_err());
    }

    #[test]
    fn paired_halves_match_independent_native_wave64_references() {
        let input = std::array::from_fn(|lane| lane as u32 * 17 + 3);
        let fixed = model_fixed_wave32(&input);
        let native = model_native_wave64(&input);
        assert_eq!(fixed, native);
        assert_eq!([fixed.1[0].epoch, fixed.1[1].epoch], [2, 3]);
        assert_eq!([fixed.1[0].pc, fixed.1[1].pc], [0x10, 0x20]);
        assert_ne!(&fixed.0[..64], &fixed.0[64..]);
    }

    #[test]
    fn validation_shader_has_only_unconditional_rendezvous() {
        let hlsl = validation_hlsl();
        assert_eq!(
            hlsl.matches("DeviceMemoryBarrierWithGroupSync();").count(),
            3
        );
        assert!(!hlsl.contains("groupshared") && !hlsl.contains("xcl_lds"));
        assert!(!hlsl.contains("if (epoch < wave + 2u) DeviceMemoryBarrier"));
        if super::super::dxc::location().is_ok() {
            super::super::dxc::compile(&hlsl, "main", "cs_6_6", &[])
                .unwrap_or_else(|error| panic!("{error}\n{hlsl}"));
        }
    }

    #[test]
    fn native_wave64_translation_does_not_include_dispatcher_foundation() {
        let code =
            crate::gpu::rdna2::encode("V_MOV_B32", "ENC_VOP1", &[("VDST", 0), ("SRC0", 128)], None)
                .unwrap();
        let translated = super::super::translate(
            &code,
            &super::super::Options {
                mode: super::super::Mode::Mesh(super::super::MsInterface {
                    threads: GROUP_THREADS,
                    max_vertices: 1,
                    max_primitives: 1,
                    ..Default::default()
                }),
                wave64: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!translated.hlsl.contains("record_base"));
        assert!(!translated.hlsl.contains("DeviceMemoryBarrierWithGroupSync"));
    }

    #[test]
    fn gpu_dispatch_matches_native_wave64_reference() {
        if let Err(reason) = super::super::runner::adapter() {
            println!("GPU unavailable – skipping ({reason})");
            super::super::runner::skip_test("no GPU");
            return;
        }
        if super::super::dxc::location().is_err() {
            super::super::runner::skip_test("no DXC");
            return;
        }
        let hlsl = validation_hlsl();
        let dxil = super::super::dxc::compile(&hlsl, "main", "cs_6_6", &[])
            .unwrap_or_else(|error| panic!("{error}\n{hlsl}"));
        let stride = MIN_RECORD_BYTES.next_multiple_of(RECORD_ALIGNMENT);
        let layout = ScratchLayout::new(0, stride, 1).unwrap();
        let memory = vec![0u32; layout.total_bytes as usize / 4];
        // The runner has a ten-second fence timeout, so divergent rendezvous
        // becomes a deterministic failure rather than an indefinite hang.
        let memory = super::super::runner::dispatch(&dxil, [1, 1, 1], &[0], &memory).unwrap();
        let input = std::array::from_fn(|lane| lane as u32 + 1);
        let (expected_values, expected_states) = model_native_wave64(&input);
        let actual_values: [u32; GROUP_THREADS as usize] = std::array::from_fn(|thread| {
            memory[layout.result_offset(0, thread as u32).unwrap() as usize / 4]
        });
        let actual_states: [LogicalWaveState; 2] = std::array::from_fn(|wave| {
            let word = layout.state_offset(0, wave as u32).unwrap() as usize / 4;
            LogicalWaveState {
                pc: memory[word],
                exec: [memory[word + 1], memory[word + 2]],
                vcc: [memory[word + 3], memory[word + 4]],
                scc: memory[word + 5],
                epoch: memory[word + 6],
                scalar: {
                    let mut scalar = [0u32; SCALAR_REGISTERS as usize];
                    scalar.copy_from_slice(
                        &memory[word + STATE_HEADER_WORDS as usize..word + STATE_WORDS as usize],
                    );
                    scalar
                },
            }
        });
        assert_eq!(actual_values, expected_values);
        assert_eq!(actual_states, expected_states);
    }
}
