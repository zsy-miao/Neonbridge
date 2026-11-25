//! XAPU decodes each voice into 48 kHz interleaved float PCM.
use opus_pure::{Error, OpusDecoder, Result, packet};

pub(crate) fn packet_samples(packet: &[u8]) -> Result<usize> {
    packet::samples_48k(packet)
}

pub(crate) struct Decoder(OpusDecoder);

impl Decoder {
    pub(crate) fn new(channels: usize) -> Result<Self> {
        OpusDecoder::new(48_000, channels).map(Self)
    }

    pub(crate) fn reset(&mut self) -> Result<()> {
        self.0.reset_state()
    }

    /// Return samples per channel. Keep one decoder per voice so consecutive
    /// packets share their prediction and overlap history.
    pub(crate) fn decode_interleaved(
        &mut self,
        packet: &[u8],
        output: &mut [f32],
    ) -> Result<usize> {
        let frames = packet_samples(packet)?;
        let samples = frames * self.0.channels();
        let output = output
            .get_mut(..samples)
            .ok_or(Error::InvalidArgument("short PCM output"))?;
        self.0.decode(packet, frames, output)
    }
}
