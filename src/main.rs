use clap::{Command, CommandFactory, Parser};
use clap_complete::{generate, Generator};
use irongrp::analyse::analyse_grp;
use irongrp::grp::{grp_to_png, png_to_grp};
use irongrp::error::InFile;
use irongrp::{Cli, Commands, Error, Result};
use log::{error, info};
use simplelog::{ColorChoice, CombinedLogger, Config, TermLogger, TerminalMode};
use std::io::stdout;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

fn main() -> ExitCode {
    let cli = Cli::parse();
    CombinedLogger::init(
        vec![
            TermLogger::new(cli.log_level.clone().into(), Config::default(), TerminalMode::Mixed, ColorChoice::Auto),
        ]
    ).unwrap();

    match run(&cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: &Commands) -> Result<()> {
    let start_time = SystemTime::now();

    match command {
        Commands::GrpToPng(args) => {
            require_existing_file(&args.input)?;
            std::fs::create_dir_all(&args.output).in_file(&args.output)?;

            grp_to_png(args)?;
            info!("Conversion complete in {} ms", time_elapsed(start_time));
        },

        Commands::PngToGrp(args) => {
            if Path::new(&args.output).is_dir() {
                return Err(Error::InvalidArgument(format!(
                    "Output path '{}' is a directory; expected a file path", args.output,
                )));
            }

            png_to_grp(args)?;
            info!("Wrote GRP in {} ms to {}", time_elapsed(start_time), args.output);
        },

        Commands::Analyse(args) => {
            require_existing_file(&args.input)?;

            analyse_grp(args)?;
            info!("Analysis complete in {} ms", time_elapsed(start_time));
        },

        Commands::Completions { shell } => {
            // The completion script is written to stdout, so the status message goes to stderr
            let mut cmd = Cli::command();
            eprintln!("Generating completions for {shell:?}...");
            print_completions(*shell, &mut cmd);
        },
    }
    Ok(())
}

fn require_existing_file(path: &str) -> Result<()> {
    let p = Path::new(path);
    if !p.exists() || p.is_dir() {
        return Err(Error::InvalidArgument(format!(
            "Input path '{}' is not an existing file", path,
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
