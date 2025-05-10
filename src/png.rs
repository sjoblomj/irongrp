use crate::grp::{GrpFrame, GrpType, EXTENDED_IMAGE_WIDTH};
use crate::palpng::{draw_image_to_pixel_buffer, read_png, save_pixel_buffer_to_image_file, ImageWithMetadata};
use crate::{log, Args, LogLevel, UNCOMPRESSED_FILENAME, WAR1_FILENAME};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::ErrorKind;

pub fn render_and_save_frames_to_png(
    frames: &[GrpFrame],
    palette: &Vec<[u8; 3]>,
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
        log(LogLevel::Debug, &format!(
            "Saving all frames as one PNG. Columns: {}, max-frame-size: {}x{}, requested max width: {}",
            cols, max_frame_width, max_frame_height, args.max_width.unwrap_or(0),
        ));

        // The user has requested a maximum width in pixels,
        // so we might need to adjust the number of columns down.
        if let Some(max_w) = args.max_width {
            if max_w > max_frame_width && cols * max_frame_width > max_w {
                cols = (max_w as f64 / max_frame_width as f64).floor() as u32;
                log(LogLevel::Debug, &format!("Adjusted number of columns to: {}", cols));
            } else if max_w < max_frame_width {
                cols = 1;
                log(LogLevel::Debug, &format!(
                    "The requested max-width, {}, is smaller than one frame. The resulting image \
                    will have 1 column and it will be {} pixels wide.",
                    max_w, max_frame_width));
            }
        }

        let canvas_width = cols * max_frame_width;
        let canvas_height = (frames.len() as f64 / cols as f64).ceil() as u32 * max_frame_height;

        let mut buffer: Vec<u8> = vec![];
        for (i, frame) in frames.iter().enumerate() {
            let col = (i as u32) % cols;
            let row = (i as u32) / cols;
            let base_x = col * max_frame_width;
            let base_y = row * max_frame_height;

            let temp_img = image_to_buffer(frame, &palette, max_frame_width, max_frame_height, args.use_transparency)?;

            for y in 0..max_frame_height {
                for x in 0..max_frame_width {
                    let dst_index = ((base_y + y) * canvas_width + (base_x + x)) as usize * if args.use_transparency { 4 } else { 3 };
                    let src_index = (y * max_frame_width + x) as usize * if args.use_transparency { 4 } else { 3 };
                    buffer[dst_index..dst_index + if args.use_transparency { 4 } else { 3 }]
                        .copy_from_slice(&temp_img[src_index..src_index  + if args.use_transparency { 4 } else { 3 }]);
                }
            }
        }

        let output_path = format!("{}/all_frames.png", args.output_path.as_deref().unwrap());
        save_pixel_buffer_to_image_file(buffer, &output_path, args.use_transparency, canvas_width, canvas_height)?;
        log(LogLevel::Info, &format!("Saved all frames to {}", output_path));

    } else {
        // Non-tiled mode - save each frame as a separate image.

        // The following two HashMaps are used for printing duplicates
        // Map: image_data_offset -> list of frame indices
        let mut offset_map: HashMap<u32, Vec<usize>> = HashMap::new();
        // Map: image hash -> list of frame indices
        let mut image_hash_map: HashMap<u64, Vec<usize>> = HashMap::new();

        for (i, frame) in frames.iter().enumerate() {
            if args.frame_number == Some(i as u16) {
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
            save_pixel_buffer_to_image_file(buffer, &output_path, args.use_transparency, max_frame_width, max_frame_height)?;
            log(LogLevel::Info, &format!("Saved frame {:2} to {}", i, output_path));
        }

        let mut offset_duplicates_vec: Vec<(&u32, &Vec<usize>)> = offset_map
            .iter()
            .filter(|(_, indices)| indices.len() > 1)
            .collect();
        // Sort by the lowest frame index in each group
        offset_duplicates_vec.sort_by_key(|(_, indices)| *indices.iter().min().unwrap());

        let mut offset_duplicates: HashSet<usize> = HashSet::new();
        for (_, indices) in offset_duplicates_vec {
            log(LogLevel::Info, &format!("Identical frames: {:?}", indices));
            offset_duplicates.extend(indices);
        }

        for (_, indices) in &image_hash_map {
            if indices.len() > 1 {
                let overlap = indices.iter().any(|idx| offset_duplicates.contains(idx));
                if !overlap {
                    log(LogLevel::Info, &format!(
                        "Identical frames with duplicated image data in GRP: {:?}", indices,
                    ));
                }
            }
        }
    }

    Ok(())
}

fn image_to_buffer(
    frame: &GrpFrame,
    palette: &Vec<[u8; 3]>,
    max_frame_width:  u32,
    max_frame_height: u32,
    use_transparency: bool,
) -> Result<Vec<u8>, std::io::Error> {

    let width = if frame.image_data.grp_type == GrpType::UncompressedExtended {
        frame.width as u32 + EXTENDED_IMAGE_WIDTH as u32
    } else {
        frame.width as u32
    };

    let image = ImageWithMetadata {
        x_offset: frame.x_offset as u32,
        y_offset: frame.y_offset as u32,
        width,
        height:   frame.height as u32,
        original_width:  max_frame_width,
        original_height: max_frame_height,
        image_data: frame.image_data.converted_pixels.clone(),
    };

    let buffer = draw_image_to_pixel_buffer(image, &palette, use_transparency)?;
    Ok(buffer)
}

pub fn png_to_pixels(png_file_name: &str, palette: &Vec<[u8; 3]>) -> std::io::Result<ImageWithMetadata<u8, u16>> {
    log(LogLevel::Debug, ""); // Give some space in the logs
    let png: ImageWithMetadata<u8, u16> = read_png(png_file_name, palette, true)?;

    if png.width as u32 > 2 * (u8::MAX as u32) || png.height as u32 > u8::MAX as u32 {
        return Err(std::io::Error::new(ErrorKind::InvalidInput, format!(
            "Width ({}) is above limit of {}, or height ({}) is above limit of {}",
            png.width, 2 * (u8::MAX as u32), png.height, u8::MAX,
        )))
    }
    Ok(png)
}
