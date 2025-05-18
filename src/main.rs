use clap::{Command, CommandFactory, Parser};
use clap_complete::{generate, Generator};
use irongrp::analyse::analyse_grp;
use irongrp::grp::{grp_to_png, png_to_grp};
use irongrp::{Args, OperationMode};
use log::{error, info};
use simplelog::{ColorChoice, CombinedLogger, Config, TermLogger, TerminalMode};
use std::io::stdout;
use std::path::Path;
use std::time::{Duration, SystemTime};

fn main() -> std::io::Result<()> {
    let args = Args::parse();
    CombinedLogger::init(
        vec![
            TermLogger::new(args.log_level.clone().into(), Config::default(), TerminalMode::Mixed, ColorChoice::Auto),
        ]
    ).unwrap();
    let start_time = SystemTime::now();

    if let Some(generator) = args.generator {
        let mut cmd = Args::command();
        info!("Generating completion file for {generator:?}...");
        print_completions(generator, &mut cmd);
        return Ok(());
    }

    if args.mode.is_none() {
        error!("Mode of operation must be specified!");
        std::process::exit(1);
    }
    if args.input_path.is_none() {
        error!("Input path must be specified!");
        std::process::exit(1);
    }
    let input_path = &args.input_path.clone().unwrap();

    if !args.tiled && args.max_width.is_some() {
        error!("The 'max-width' argument is only applicable when using the 'tiled' argument.");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
    }
    if args.tiled && args.frame_number.is_some() {
        error!("The 'frame-number' argument is not applicable when using the 'tiled' argument.");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
    }
    if args.mode == Some(OperationMode::PngToGrp) && args.frame_number.is_some() {
        error!("The 'frame-number' argument is not applicable when using the 'png-to-grp' mode.");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
    }
    if args.mode != Some(OperationMode::AnalyseGrp) && args.analyse_row_number.is_some() {
        error!("The 'analyse-row-number' argument is only applicable when using the 'analyse-grp' mode.");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
    }
    if args.frame_number.is_none() && args.analyse_row_number.is_some() {
        error!("The 'analyse-row-number' argument is only applicable when used together with the 'frame-number' argument.");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
    }

    match args.mode.clone().unwrap() {
        OperationMode::GrpToPng => {
            let output_path = &args.output_path
                .as_ref()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing --output-path argument"))?;
            let p = Path::new(input_path);
            if !p.exists() || p.is_dir() {
                error!("Invalid input path, please provide a file path to a GRP file.");
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
            }
            if (&args.pal_path).is_none() {
                error!("Invalid pal-path, please provide a file path to a Palette file.");
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing --pal-path argument"));
            }
            std::fs::create_dir_all(output_path)?;

            grp_to_png(&args)?;
            info!("Conversion complete in {} ms", time_elapsed(start_time));
        },

        OperationMode::PngToGrp => {
            let output_path = &args.output_path
                .as_ref()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing --output-path argument"))?;

            let p = Path::new(output_path);
            if p.exists() && p.is_dir() {
                error!("The given output path is a directory; please provide a file path instead.");
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
            }
            if (&args.pal_path).is_none() {
                error!("Invalid pal-path, please provide a file path to a Palette file.");
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing --pal-path argument"));
            }

            png_to_grp(&args)?;
            info!("Wrote GRP in {} ms to {}", time_elapsed(start_time), output_path);
        },

        OperationMode::AnalyseGrp => {
            let p = Path::new(input_path);
            if !p.exists() || p.is_dir() {
                error!("Invalid input path, please provide a file path to a GRP file");
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
            }

            analyse_grp(&args)?;
            info!("Analysis complete in {} ms", time_elapsed(start_time));
        },
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
