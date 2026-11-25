//! Son de la Xbox Series X : fichiers RIFF XMA2 décodés en PCM 16 bits.
//!
//! Le décodage est celui de la crate `xma` de TomBox (omonkeyz/xbox-emulator, LGPL-2.1+), un
//! portage en Rust du décodeur WMA Pro / XMA de FFmpeg (`libavcodec/wmaprodec.c`). Ce module lit
//! l'en-tête XMA2WAVEFORMATEX (format 0x0166, documentation publique de Microsoft) et écrit un WAV.

/// Format WAVE de XMA2 (XMA2WAVEFORMATEX).
pub const XMA2_FORMAT_TAG: u16 = 0x0166;
pub const PACKET_BYTES: usize = 2048;
const PCM_FORMAT_TAG: u16 = 1;
const BITS_PER_SAMPLE: u16 = 16;

/// Paramètres d'un flux XMA2 lus dans le bloc `fmt `.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Xma2Format {
    pub channels: u16,
    pub sample_rate: u32,
    pub streams: u16,
}

/// Contenu utile d'un fichier RIFF XMA2.
#[derive(Debug)]
pub struct Xma2File<'a> {
    pub format: Xma2Format,
    pub data: &'a [u8],
}

fn u16_at(b: &[u8], offset: usize) -> Result<u16, String> {
    b.get(offset..offset + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| "fichier tronqué".into())
}

fn u32_at(b: &[u8], offset: usize) -> Result<u32, String> {
    b.get(offset..offset + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| "fichier tronqué".into())
}

/// Lit un fichier RIFF/WAVE dont le bloc `fmt ` est un XMA2WAVEFORMATEX.
pub fn parse(file: &[u8]) -> Result<Xma2File<'_>, String> {
    if file.get(..4) != Some(b"RIFF") || file.get(8..12) != Some(b"WAVE") {
        return Err("pas un fichier RIFF/WAVE".into());
    }
    let (mut format, mut data, mut offset) = (None, None, 12);
    while offset + 8 <= file.len() {
        let size = u32_at(file, offset + 4)? as usize;
        let body = file.get(offset + 8..offset + 8 + size).ok_or("bloc RIFF tronqué")?;
        match &file[offset..offset + 4] {
            b"fmt " => {
                let tag = u16_at(body, 0)?;
                if tag != XMA2_FORMAT_TAG {
                    return Err(format!("format 0x{tag:04X}, XMA2 (0x0166) attendu"));
                }
                // WAVEFORMATEX (18 octets), puis NumStreams.
                format = Some(Xma2Format { channels: u16_at(body, 2)?, sample_rate: u32_at(body, 4)?,
                                           streams: u16_at(body, 18)? });
            }
            b"data" => data = Some(body),
            _ => {}
        }
        offset += 8 + size + (size & 1);
    }
    Ok(Xma2File { format: format.ok_or("bloc fmt absent")?, data: data.ok_or("bloc data absent")? })
}

/// Décode des paquets XMA2 de 2 Kio en échantillons PCM 16 bits entrelacés.
pub fn decode(format: Xma2Format, data: &[u8]) -> Result<Vec<i16>, String> {
    if format.streams != 1 {
        return Err(format!("{} flux XMA2 : seuls les fichiers à un flux (1 ou 2 canaux) sont pris en charge",
                           format.streams));
    }
    let mut stream = xma::Xma2Stream::new(format.sample_rate, format.channels as usize)
        .map_err(|error| format!("configuration XMA2 refusée : {error:?}"))?;
    let mut samples = Vec::new();
    for packet in data.chunks(PACKET_BYTES) {
        stream.decode_packet(packet, &mut samples);
    }
    Ok(samples)
}

/// Fichier WAV PCM 16 bits.
pub fn wav(channels: u16, sample_rate: u32, samples: &[i16]) -> Vec<u8> {
    let data_bytes = (samples.len() * 2) as u32;
    let block_align = channels * BITS_PER_SAMPLE / 8;
    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&PCM_FORMAT_TAG.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * block_align as u32).to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xma2_riff(channels: u16, rate: u32, streams: u16, data: &[u8]) -> Vec<u8> {
        let mut fmt = Vec::new();
        for value in [XMA2_FORMAT_TAG, channels] {
            fmt.extend_from_slice(&value.to_le_bytes());
        }
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&[0; 8]); // octets par seconde, alignement, bits
        fmt.extend_from_slice(&34u16.to_le_bytes()); // cbSize
        fmt.extend_from_slice(&streams.to_le_bytes());
        fmt.extend_from_slice(&[0; 32]);
        let mut out = b"RIFF\0\0\0\0WAVE".to_vec();
        for (kind, body) in [(b"fmt ", &fmt[..]), (b"data", data)] {
            out.extend_from_slice(kind);
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(body);
        }
        out
    }

    #[test]
    fn parses_the_xma2_header() {
        let file = xma2_riff(2, 48_000, 1, &[0; PACKET_BYTES]);
        let parsed = parse(&file).unwrap();
        assert_eq!(parsed.format, Xma2Format { channels: 2, sample_rate: 48_000, streams: 1 });
        assert_eq!(parsed.data.len(), PACKET_BYTES);
    }

    #[test]
    fn rejects_other_formats_and_multistream_files() {
        let mut pcm = xma2_riff(2, 48_000, 1, &[]);
        pcm[20..22].copy_from_slice(&1u16.to_le_bytes()); // format PCM
        assert!(parse(&pcm).unwrap_err().contains("0x0001"));
        assert!(parse(b"nope").is_err());
        let format = Xma2Format { channels: 6, sample_rate: 48_000, streams: 3 };
        assert!(decode(format, &[]).unwrap_err().contains("3 flux"));
    }

    #[test]
    fn garbage_packets_decode_without_crash() {
        let format = Xma2Format { channels: 1, sample_rate: 44_100, streams: 1 };
        let samples = decode(format, &[0xA5; PACKET_BYTES * 2]).unwrap();
        assert!(samples.len() % format.channels as usize == 0);
    }

    #[test]
    fn writes_a_pcm_wav_header() {
        let file = wav(2, 48_000, &[1, -1, 2, -2]);
        assert_eq!(&file[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(file[40..44].try_into().unwrap()), 8);
        assert_eq!(u16::from_le_bytes([file[22], file[23]]), 2);
        assert_eq!(file.len(), 44 + 8);
    }
}
