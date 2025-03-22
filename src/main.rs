use clap::Parser;
use irongrp::{render_and_save_frames_to_png, read_palette, read_grp_header, read_grp_frames, LOG_LEVEL, log, LogLevel, Args};

fn main() -> std::io::Result<()> {
    let args = Args::parse();

    LOG_LEVEL.set(args.log_level.clone()).expect("Failed to set log level");

    if !args.tiled && args.max_width.is_some() {
        log(LogLevel::Error, "The 'max-width' argument is only applicable when using the 'tiled' argument.");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
    }

    std::fs::create_dir_all(&args.output_dir)?;

    let mut file = std::fs::File::open(&args.grp_path)?;
    let palette  = read_palette(&args.pal_path)?;
    let header   = read_grp_header(&mut file)?;
    let frames   = read_grp_frames(&mut file, header.frame_count as usize)?;

    render_and_save_frames_to_png(
        &frames,
        &palette,
        header.max_width as u32,
        header.max_height as u32,
        &args,
    )?;

    log(LogLevel::Info, "Conversion complete");
    Ok(())
}

