use std::path::Path;
use clap::Parser;
use irongrp::grp::{grp_to_png, png_to_grp};
use irongrp::analyse::analyse_grp;
use irongrp::{LOG_LEVEL, log, LogLevel, OperationMode, Args};

fn main() -> std::io::Result<()> {
    let args = Args::parse();
    LOG_LEVEL.set(args.log_level.clone()).expect("Failed to set log level");

    if !args.tiled && args.max_width.is_some() {
        log(LogLevel::Error, "The 'max-width' argument is only applicable when using the 'tiled' argument.");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
    }

    if args.mode == OperationMode::GrpToPng {
        let output_path = &args.output_path
            .as_ref()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing --output-path argument"))?;
        let p = Path::new(&args.input_path);
        if !p.exists() || p.is_dir() {
            log(LogLevel::Error, "Invalid input path, please provide a file path to a GRP file.");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
        }
        if (&args.pal_path).is_none() {
            log(LogLevel::Error, "Invalid pal-path, please provide a file path to a Palette file.");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing --pal-path argument"));
        }
        std::fs::create_dir_all(output_path)?;

        grp_to_png(&args)?;
        log(LogLevel::Info, "Conversion complete");

    } else if args.mode == OperationMode::PngToGrp {
        let output_path = &args.output_path
            .as_ref()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing --output-path argument"))?;

        let p = Path::new(output_path);
        if p.exists() && p.is_dir() {
            log(LogLevel::Error, "Output path is a directory, please provide a file path.");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
        }
        if (&args.pal_path).is_none() {
            log(LogLevel::Error, "Invalid pal-path, please provide a file path to a Palette file.");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing --pal-path argument"));
        }
 
        png_to_grp(&args)?;
        log(LogLevel::Info, &format!("Wrote GRP to {}", output_path));

    } else if args.mode == OperationMode::AnalyseGrp {
        let p = Path::new(&args.input_path);
        if !p.exists() || p.is_dir() {
            log(LogLevel::Error, "Invalid input path, please provide a file path to a GRP file.");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
        }

        analyse_grp(&args)?;
        log(LogLevel::Info, "Analysis complete");

    } else {
        log(LogLevel::Error, "Invalid mode!");
    }
    Ok(())
}
