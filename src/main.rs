use clap::{Command, CommandFactory, Parser};
use clap_complete::{generate, Generator};
use irongrp::analyse::analyse_grp;
use irongrp::grp::{grp_to_png, png_to_grp};
use irongrp::error::InFile;
use irongrp::{Args, Error, OperationMode, Result};
use log::{error, info};
use simplelog::{ColorChoice, CombinedLogger, Config, TermLogger, TerminalMode};
use std::io::stdout;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

fn main() -> ExitCode {
    let args = Args::parse();
    CombinedLogger::init(
        vec![
            TermLogger::new(args.log_level.clone().into(), Config::default(), TerminalMode::Mixed, ColorChoice::Auto),
        ]
    ).unwrap();

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<()> {
    let start_time = SystemTime::now();

    if let Some(generator) = args.generator {
        let mut cmd = Args::command();
        info!("Generating completion file for {generator:?}...");
        print_completions(generator, &mut cmd);
        return Ok(());
    }

    // Argument-combination checks that clap's derive attributes can't express, because they
    // depend on the *value* of --mode rather than just its presence. All other combination
    // constraints (requires / conflicts_with / required_unless_present / required_if_eq_any)
    // are declared on the Args struct in lib.rs and enforced by clap at parse time.
    validate_value_dependencies(args);

    // After clap parsing + validate_value_dependencies, --mode and --input-path are guaranteed
    // to be Some(_), and --output-path is Some(_) whenever the mode requires it.
    let mode = args.mode.as_ref().unwrap();
    let input_path = args.input_path.as_deref().unwrap();

    match mode {
        OperationMode::GrpToPng => {
            let output_path = args.output_path.as_deref().unwrap();
            require_existing_file(input_path)?;
            std::fs::create_dir_all(output_path).in_file(output_path)?;

            grp_to_png(args)?;
            info!("Conversion complete in {} ms", time_elapsed(start_time));
        },

        OperationMode::PngToGrp => {
            let output_path = args.output_path.as_deref().unwrap();
            if Path::new(output_path).is_dir() {
                return Err(Error::InvalidArgument(format!(
                    "'--output-path' value '{}' is a directory; expected a file path", output_path,
                )));
            }

            png_to_grp(args)?;
            info!("Wrote GRP in {} ms to {}", time_elapsed(start_time), output_path);
        },

        OperationMode::AnalyseGrp => {
            require_existing_file(input_path)?;

            analyse_grp(args)?;
            info!("Analysis complete in {} ms", time_elapsed(start_time));
        },
    }
    Ok(())
}

fn validate_value_dependencies(args: &Args) {
    let mut cmd = Args::command();
    if args.mode.as_ref() == Some(&OperationMode::PngToGrp) && args.frame_number.is_some() {
        cmd.error(
            clap::error::ErrorKind::ArgumentConflict,
            "the argument '--frame-number' cannot be used with '--mode png-to-grp'",
        ).exit();
    }
    if args.mode.as_ref() != Some(&OperationMode::AnalyseGrp) && args.analyse_row_number.is_some() {
        cmd.error(
            clap::error::ErrorKind::ArgumentConflict,
            "the argument '--analyse-row-number' can only be used with '--mode analyse-grp'",
        ).exit();
    }
}

fn require_existing_file(path: &str) -> Result<()> {
    let p = Path::new(path);
    if !p.exists() || p.is_dir() {
        return Err(Error::InvalidArgument(format!(
            "'--input-path' value '{}' is not an existing file", path,
        )));
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
