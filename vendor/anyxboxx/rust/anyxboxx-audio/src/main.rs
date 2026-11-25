//! anyxboxx_xma <entrée.xma> <sortie.wav>
//!
//! Décode un fichier RIFF XMA2 (un flux, 1 ou 2 canaux) en WAV PCM 16 bits avec le décodeur
//! de TomBox (portage Rust de FFmpeg).
use std::process::ExitCode;

const USAGE: &str = "Usage : anyxboxx_xma <entrée.xma> <sortie.wav>";

fn run(args: &[String]) -> Result<String, String> {
    let [input, output] = args else {
        return Err(USAGE.into());
    };
    let file = std::fs::read(input).map_err(|error| format!("lecture de {input} : {error}"))?;
    let parsed = anyxboxx_audio::parse(&file)?;
    let samples = anyxboxx_audio::decode(parsed.format, parsed.data)?;
    let wav = anyxboxx_audio::wav(parsed.format.channels, parsed.format.sample_rate, &samples);
    std::fs::write(output, wav).map_err(|error| format!("écriture de {output} : {error}"))?;
    let frames = samples.len() / parsed.format.channels.max(1) as usize;
    Ok(format!("{output} : {frames} échantillons par canal, {} canal(aux), {} Hz", parsed.format.channels,
               parsed.format.sample_rate))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
