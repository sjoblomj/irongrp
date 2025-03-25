use std::path::Path;
use clap::Parser;
use irongrp::grp::{grp_to_png, png_to_grp};
use irongrp::{LOG_LEVEL, log, LogLevel, OperationMode, Args};

fn main() -> std::io::Result<()> {
    let args = Args::parse();

    LOG_LEVEL.set(args.log_level.clone()).expect("Failed to set log level");

    if !args.tiled && args.max_width.is_some() {
        log(LogLevel::Error, "The 'max-width' argument is only applicable when using the 'tiled' argument.");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
    }

    if args.mode == OperationMode::GrpToPng {
        let p = Path::new(&args.input_path);
        if !p.exists() || p.is_dir() {
            log(LogLevel::Error, "Invalid input path, please provide a file path to a GRP file.");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
        }
        std::fs::create_dir_all(&args.output_path)?;

        grp_to_png(&args)?;
        log(LogLevel::Info, "Conversion complete");

    } else if args.mode == OperationMode::PngToGrp {
        let p = Path::new(&args.output_path);
        if p.exists() && p.is_dir() {
            log(LogLevel::Error, "Output path is a directory, please provide a file path.");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
        }

        png_to_grp(&args)?;
        log(LogLevel::Info, &format!("Wrote GRP to {}", &args.output_path));

    } else {
        log(LogLevel::Error, "Invalid mode!");
    }
    Ok(())
}
