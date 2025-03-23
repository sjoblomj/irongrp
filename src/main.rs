use clap::Parser;
use irongrp::png::render_and_save_frames_to_png;
use irongrp::{list_png_files, read_palette, read_grp_header, read_grp_frames, LOG_LEVEL, log, LogLevel, OperationMode, Args};

fn main() -> std::io::Result<()> {
    let args = Args::parse();

    LOG_LEVEL.set(args.log_level.clone()).expect("Failed to set log level");

    if !args.tiled && args.max_width.is_some() {
        log(LogLevel::Error, "The 'max-width' argument is only applicable when using the 'tiled' argument.");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
    }

    std::fs::create_dir_all(&args.output_dir)?;

    let palette = read_palette(&args.pal_path)?;

    if args.mode == OperationMode::Grp2Png {
        let mut f  = std::fs::File::open(&args.input_path)?;
        let header = read_grp_header(&mut f)?;
        let frames = read_grp_frames(&mut f, header.frame_count as usize)?;

        render_and_save_frames_to_png(
            &frames,
            &palette,
            header.max_width as u32,
            header.max_height as u32,
            &args,
        )?;

        log(LogLevel::Info, "Conversion complete");

    } else if args.mode == OperationMode::Png2Grp {
        let png_files = list_png_files(&args.input_path)?;
        //let grp_frames = files_to_grp(png_files, &palette)?;
        log(LogLevel::Info, "Created GRP");

    } else {
        log(LogLevel::Error, "Invalid mode!");
    }
    Ok(())
}

