use clap::{Command, CommandFactory, Parser};
use clap_complete::{generate, Generator};
use irongrp::analyse::analyse_grp;
use irongrp::grp::{grp_to_png, png_to_grp};
use irongrp::{log, Args, LogLevel, OperationMode, LOG_LEVEL};
use std::io::stdout;
use std::path::Path;
use std::time::{Duration, SystemTime};

fn main() -> std::io::Result<()> {
    let args = Args::parse();
    LOG_LEVEL.set(args.log_level.clone()).expect("Failed to set log level");
    let start_time = SystemTime::now();

    if let Some(generator) = args.generator {
        let mut cmd = Args::command();
        eprintln!("Generating completion file for {generator:?}...");
        print_completions(generator, &mut cmd);
        return Ok(());
    }

    if args.mode.is_none() {
        eprintln!("Mode of operation must be specified!");
        std::process::exit(1);
    }
    if args.input_path.is_none() {
        eprintln!("Input path must be specified!");
        std::process::exit(1);
    }
    let input_path = &args.input_path.clone().unwrap();

    if !args.tiled && args.max_width.is_some() {
        log(LogLevel::Error, "The 'max-width' argument is only applicable when using the 'tiled' argument.");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
    }
    if args.tiled && args.frame_number.is_some() {
        log(LogLevel::Error, "The 'frame-number' argument is not applicable when using the 'tiled' argument.");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
    }
    if args.mode == Some(OperationMode::PngToGrp) && args.frame_number.is_some() {
        log(LogLevel::Error, "The 'frame-number' argument is not applicable when using the 'png-to-grp' mode.");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
    }
    if args.mode != Some(OperationMode::AnalyseGrp) && args.analyse_row_number.is_some() {
        log(LogLevel::Error, "The 'analyse-row-number' argument is only applicable when using the 'analyse-grp' mode.");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
    }
    if args.frame_number.is_none() && args.analyse_row_number.is_some() {
        log(LogLevel::Error, "The 'analyse-row-number' argument is only applicable when used together with the 'frame-number' argument.");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
    }

    if args.mode == Some(OperationMode::GrpToPng) {
        let output_path = &args.output_path
            .as_ref()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing --output-path argument"))?;
        let p = Path::new(input_path);
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
        log(LogLevel::Info, &format!("Conversion complete in {} ms", time_elapsed(start_time)));

    } else if args.mode == Some(OperationMode::PngToGrp) {
        let output_path = &args.output_path
            .as_ref()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing --output-path argument"))?;

        let p = Path::new(output_path);
        if p.exists() && p.is_dir() {
            log(LogLevel::Error, "The given output path is a directory; please provide a file path instead.");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
        }
        if (&args.pal_path).is_none() {
            log(LogLevel::Error, "Invalid pal-path, please provide a file path to a Palette file.");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing --pal-path argument"));
        }
 
        png_to_grp(&args)?;
        log(LogLevel::Info, &format!("Wrote GRP in {} ms to {}", time_elapsed(start_time), output_path));

    } else if args.mode == Some(OperationMode::AnalyseGrp) {
        let p = Path::new(input_path);
        if !p.exists() || p.is_dir() {
            log(LogLevel::Error, "Invalid input path, please provide a file path to a GRP file.");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
        }

        analyse_grp(&args)?;
        log(LogLevel::Info, &format!("Analysis complete in {} ms", time_elapsed(start_time)));

    } else {
        log(LogLevel::Error, "Invalid mode!");
    }
    Ok(())
}

fn time_elapsed(start_time: SystemTime) -> u128 {
    start_time.elapsed().unwrap_or_else(|_| Duration::new(0, 0)).as_millis()
}

fn print_completions<G: Generator>(generator: G, cmd: &mut Command) {
    generate(
        generator,
        cmd,
        cmd.get_name().to_string(),
        &mut stdout(),
    );
}
