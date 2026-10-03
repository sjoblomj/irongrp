use crate::grp::{GrpFrame, GrpType, EXTENDED_IMAGE_WIDTH};
use crate::{palpngrs_to_io_error, Args, UNCOMPRESSED_FILENAME, WAR1_FILENAME};
use log::{debug, info};
use palpngrs::{draw_image_to_pixel_buffer, read_png, save_pixels_to_image_file, Offset, Palette0Pixels, PalettizedImageWithMetadata, Size};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::ErrorKind;

pub fn render_and_save_frames_to_png(
    frames: &[GrpFrame],
    palette: &[[u8; 3]],
    max_frame_width:  u32,
    max_frame_height: u32,
    args: &Args,
) -> std::io::Result<()> {
    if args.tiled && args.frame_number.is_none() {
        // Tiled mode, so we need to draw all frames into one image.
        // Attempt to set the number of columns to sqrt(number of frames), so e.g., if there
        // are 25 frames, we will attempt to create a 5x5 image.
        // If the user has requested a max_width, then scale down to try to accommodate for that.
        // So, if there are 25 frames, but the user has requested a max_width that only fits
        // 3 frames, then the resulting image would be 3x9
        let mut cols = (frames.len() as f64).sqrt().floor() as u32;
        debug!(
            "Saving all frames as one PNG. Columns: {}, max-frame-size: {}x{}, requested max width: {}",
            cols, max_frame_width, max_frame_height, args.max_width.unwrap_or(0),
        );

        // The user has requested a maximum width in pixels,
        // so we might need to adjust the number of columns down.
        if let Some(max_w) = args.max_width {
            if max_w > max_frame_width && cols * max_frame_width > max_w {
                cols = (max_w as f64 / max_frame_width as f64).floor() as u32;
                debug!("Adjusted number of columns to: {}", cols);
            } else if max_w < max_frame_width {
                cols = 1;
                debug!(
                    "The requested max-width, {}, is smaller than one frame. The resulting image \
                    will have 1 column and it will be {} pixels wide.",
                    max_w, max_frame_width
                );
            }
        }

        let canvas_width = cols * max_frame_width;
        let canvas_height = (frames.len() as f64 / cols as f64).ceil() as u32 * max_frame_height;

        let pixel_length: usize = if args.use_transparency { 4 } else { 3 }; // RGBA or RGB
        let mut buffer = vec![0u8; pixel_length * (canvas_width * canvas_height) as usize];

        for (i, frame) in frames.iter().enumerate() {
            let col = (i as u32) % cols;
            let row = (i as u32) / cols;
            let base_x = col * max_frame_width;
            let base_y = row * max_frame_height;

            let temp_img = image_to_buffer(frame, &palette, max_frame_width, max_frame_height, args.use_transparency)?;

            for y in 0..max_frame_height {
                for x in 0..max_frame_width {
                    let dst_index = ((base_y + y) * canvas_width + (base_x + x)) as usize * pixel_length;
                    let src_index = (y * max_frame_width + x) as usize * pixel_length;
                    buffer[dst_index..dst_index + pixel_length]
                        .copy_from_slice(&temp_img[src_index..src_index  + pixel_length]);
                }
            }
        }

        let output_path = format!("{}/all_frames.png", args.output_path.as_deref().unwrap());
        save_pixels_to_image_file(buffer, &output_path, args.use_transparency, canvas_width, canvas_height)
            .map_err(palpngrs_to_io_error)?;
        info!("Saved all frames to {}", output_path);

    } else {
        // Non-tiled mode - save each frame as a separate image.

        // The following two HashMaps are used for printing duplicates
        // Map: image_data_offset -> list of frame indices
        let mut offset_map: HashMap<u32, Vec<usize>> = HashMap::new();
        // Map: image hash -> list of frame indices
        let mut image_hash_map: HashMap<u64, Vec<usize>> = HashMap::new();

        for (i, frame) in frames.iter().enumerate() {
            if args.frame_number.is_some() && args.frame_number != Some(i as u16) {
                continue;
            }
            offset_map.entry(frame.image_data_offset)
                .or_default()
                .push(i);

            let buffer = image_to_buffer(frame, &palette, max_frame_width, max_frame_height, args.use_transparency)?;

            let mut hasher = DefaultHasher::new();
            buffer.hash(&mut hasher); // Hash the raw RGB(A) buffer
            let image_hash = hasher.finish();

            image_hash_map.entry(image_hash)
                .or_default()
                .push(i);

            let grp_type = if frame.image_data.grp_type == GrpType::Normal {
                ""
            } else if frame.image_data.grp_type == GrpType::War1 {
                &format!("{}_", WAR1_FILENAME)
            } else {
                &format!("{}_", UNCOMPRESSED_FILENAME)
            };

            let output_path = format!("{}/{}frame_{:03}.png", args.output_path.as_deref().unwrap(), grp_type, i);
            save_pixels_to_image_file(buffer, &output_path, args.use_transparency, max_frame_width, max_frame_height)
                .map_err(palpngrs_to_io_error)?;
            info!("Saved frame {:2} to {}", i, output_path);
        }

        let mut offset_duplicates_vec: Vec<(&u32, &Vec<usize>)> = offset_map
            .iter()
            .filter(|(_, indices)| indices.len() > 1)
            .collect();
        // Sort by the lowest frame index in each group
        offset_duplicates_vec.sort_by_key(|(_, indices)| *indices.iter().min().unwrap());

        let mut offset_duplicates: HashSet<usize> = HashSet::new();
        for (_, indices) in offset_duplicates_vec {
            info!("Identical frames: {:?}", indices);
            offset_duplicates.extend(indices);
        }

        for (_, indices) in &image_hash_map {
            if indices.len() > 1 {
                let overlap = indices.iter().any(|idx| offset_duplicates.contains(idx));
                if !overlap {
                    info!(
                        "Identical frames with duplicated image data in GRP: {:?}", indices,
                    );
                }
            }
        }
    }

    Ok(())
}

fn image_to_buffer(
    frame: &GrpFrame,
    palette: &[[u8; 3]],
    max_frame_width:  u32,
    max_frame_height: u32,
    use_transparency: bool,
) -> Result<Vec<u8>, std::io::Error> {

    let width = if frame.image_data.grp_type == GrpType::UncompressedExtended {
        frame.width as u32 + EXTENDED_IMAGE_WIDTH as u32
    } else {
        frame.width as u32
    };

    let image = PalettizedImageWithMetadata::new(
        Offset::new(frame.x_offset as u32, frame.y_offset as u32),
        Size::new(width, frame.height as u32),
        Size::new(max_frame_width, max_frame_height),
        frame.image_data.converted_pixels.clone(),
    );

    let buffer = draw_image_to_pixel_buffer(image, palette, use_transparency)
        .map_err(palpngrs_to_io_error)?;
    Ok(buffer)
}

pub fn png_to_pixels(png_file_name: &str, palette: &[[u8; 3]]) -> std::io::Result<PalettizedImageWithMetadata<u8, u16>> {
    debug!(""); // Give some space in the logs
    // PNGs exported without --use-transparency have their transparent pixels drawn as palette[0],
    // so treat that colour as transparent when reading them back.
    let png: PalettizedImageWithMetadata<u8, u16> = read_png(png_file_name, palette, true, Palette0Pixels::Transparent)
        .map_err(palpngrs_to_io_error)?;

    // UncompressedExtended GRPs store width as `actual_width - EXTENDED_IMAGE_WIDTH` in a u8,
    // so the maximum representable width is EXTENDED_IMAGE_WIDTH + u8::MAX (= 511).
    let max_width = EXTENDED_IMAGE_WIDTH as u32 + u8::MAX as u32;
    if png.width as u32 > max_width || png.height as u32 > u8::MAX as u32 {
        return Err(std::io::Error::new(ErrorKind::InvalidInput, format!(
            "Width ({}) is above limit of {}, or height ({}) is above limit of {}",
            png.width, max_width, png.height, u8::MAX,
        )))
    }
    Ok(png)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::grp::ImageData;
    use crate::{CompressionType, LogLevel};
    use palpngrs::greyscale_palette;
    use std::path::Path;

    fn make_test_frame(pixel_value: u8, width: u8, height: u8) -> GrpFrame {
        GrpFrame {
            x_offset: 0,
            y_offset: 0,
            width,
            height,
            image_data_offset: 0,
            image_data: ImageData {
                row_offsets:      vec![],
                raw_row_data:     vec![],
                converted_pixels: vec![pixel_value; width as usize * height as usize],
                grp_type:         GrpType::Normal,
            },
        }
    }

    fn make_test_args(output_path: &str, frame_number: Option<u16>) -> Args {
        Args {
            input_path:         None,
            pal_path:           None,
            output_path:        Some(output_path.to_string()),
            mode:               None,
            compression_type:   CompressionType::Auto,
            tiled:              false,
            max_width:          None,
            frame_number,
            analyse_row_number: None,
            use_transparency:   false,
            log_level:          LogLevel::Info,
            generator:          None,
        }
    }

    #[test]
    fn saves_all_frames_when_no_frame_number_given() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path().to_str().unwrap();
        let palette = greyscale_palette();

        let frames = vec![
            make_test_frame(10, 4, 4),
            make_test_frame(20, 4, 4),
            make_test_frame(30, 4, 4),
        ];
        let args = make_test_args(dir, None);

        render_and_save_frames_to_png(&frames, &palette, 4, 4, &args).unwrap();

        for i in 0..frames.len() {
            let path = format!("{}/frame_{:03}.png", dir, i);
            assert!(Path::new(&path).exists(), "Expected {} to exist", path);
        }
    }

    #[test]
    fn saves_only_requested_frame_when_frame_number_given() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path().to_str().unwrap();
        let palette = greyscale_palette();

        let frames = vec![
            make_test_frame(10, 4, 4),
            make_test_frame(20, 4, 4),
            make_test_frame(30, 4, 4),
        ];
        let args = make_test_args(dir, Some(1));

        render_and_save_frames_to_png(&frames, &palette, 4, 4, &args).unwrap();

        assert!(!Path::new(&format!("{}/frame_000.png", dir)).exists());
        assert!( Path::new(&format!("{}/frame_001.png", dir)).exists());
        assert!(!Path::new(&format!("{}/frame_002.png", dir)).exists());
    }

    #[test]
    fn png_without_transparency_reads_back_with_transparent_background() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path().to_str().unwrap();
        let palette = greyscale_palette();

        // A 2x2 frame at (3, 1) on a 6x4 canvas. The background is drawn as
        // opaque palette[0] since use_transparency is false.
        let mut frame = make_test_frame(10, 2, 2);
        frame.x_offset = 3;
        frame.y_offset = 1;
        let args = make_test_args(dir, None);
        render_and_save_frames_to_png(&[frame], &palette, 6, 4, &args).unwrap();

        let png = png_to_pixels(&format!("{}/frame_000.png", dir), &palette).unwrap();
        assert_eq!((png.x_offset, png.y_offset), (3, 1));
        assert_eq!((png.width, png.height), (2, 2));
        assert_eq!(png.palettized_image, vec![10; 4]);
    }
}
