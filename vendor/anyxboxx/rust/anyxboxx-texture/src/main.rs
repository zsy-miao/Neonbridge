//! anyxboxx_texture bc7 <blocs.bin> <largeur> <hauteur> <sortie.png> [--4k-s]
//!
//! Décode une surface de blocs BC7 (ligne par ligne, ou en tuiles GFX10 SW_4K_S avec `--4k-s`)
//! et l'écrit en PNG.
use std::process::ExitCode;

use anyxboxx_texture::{decode_surface, png, tiles};

const USAGE: &str = "Usage : anyxboxx_texture bc7 <blocs.bin> <largeur> <hauteur> <sortie.png> [--4k-s]";

fn run(args: &[String]) -> Result<String, String> {
    let [command, input, width, height, output, options @ ..] = args else {
        return Err(USAGE.into());
    };
    if command != "bc7" {
        return Err(USAGE.into());
    }
    let width: usize = width.parse().map_err(|_| "largeur invalide")?;
    let height: usize = height.parse().map_err(|_| "hauteur invalide")?;
    let mut blocks = std::fs::read(input).map_err(|error| format!("lecture de {input} : {error}"))?;
    if options.iter().any(|option| option == "--4k-s") {
        blocks = tiles::linearize_4k_s(&blocks, width, height)?;
    }
    let image = decode_surface(&blocks, width, height)?;
    let file = png::encode(width, height, &image)?;
    std::fs::write(output, file).map_err(|error| format!("écriture de {output} : {error}"))?;
    Ok(format!("{output} : {width}×{height}"))
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
