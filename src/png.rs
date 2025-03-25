use crate::{LogLevel, log, Args};
use crate::grp::GrpFrame;
use image::{ImageBuffer, DynamicImage};

// Draws a frame into a raw buffer (Vec<u8>)
fn draw_frame_to_raw_buffer(
    frame: &GrpFrame,
    palette: &[[u8; 3]],
    max_width: u32,
    max_height: u32,
    use_transparency: bool,
) -> Vec<u8> {
    let mut buffer = vec![0u8; (max_width * max_height * if use_transparency { 4 } else { 3 }) as usize];

    let x_offset = frame.x_offset as u32;
    let y_offset = frame.y_offset as u32;

    for y in 0..frame.height as u32 {
        for x in 0..frame.width as u32 {
            let idx = (y * frame.width as u32 + x) as usize;
            let palette_index = frame.image_data.converted_pixels[idx] as usize;
            let color = palette[palette_index];

            let out_x = x + x_offset;
            let out_y = y + y_offset;
            let pixel_index = (out_y * max_width + out_x) as usize;

            if use_transparency {
                let base = pixel_index * 4;
                if palette_index == 0 {
                    buffer[base..base + 4].copy_from_slice(&[0, 0, 0, 0]);
                } else {
                    buffer[base..base + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
                }
            } else {
                let base = pixel_index * 3;
                buffer[base..base + 3].copy_from_slice(&[color[0], color[1], color[2]]);
            }
        }
    }

    buffer
}

pub fn render_and_save_frames_to_png(
    frames: &[GrpFrame],
    palette: &[[u8; 3]],
    max_frame_width: u32,
    max_frame_height: u32,
    args: &Args,
) -> std::io::Result<()> {
    if args.tiled {
        // Tiled mode, so we need to draw all frames into one image.
        // Attempt to set the number of columns to sqrt(number of frames), but adjust if the resulting
        // image is too wide. Thus, if there are 25 frames, we will attempt to create a 5x5 image.
        let mut cols = ((frames.len() as f64).sqrt()).floor() as u32;
        log(LogLevel::Debug, &format!(
            "Saving all frames as one PNG. Columns: {}, max-frame-size: {}x{}, requested max width: {}",
            cols,
            max_frame_width,
            max_frame_height,
            args.max_width.unwrap_or(0)
        ));

        // The user has requested a maximum width in pixels, so we need to adjust the number of columns down.
        if let Some(max_w) = args.max_width {
            if max_w > max_frame_width && cols * max_frame_width > max_w {
                cols = (max_w as f64 / max_frame_width as f64).floor() as u32;
                log(LogLevel::Debug, &format!("Adjusted number of columns to: {}", cols));
            }
        }

        let canvas_width = cols * max_frame_width;
        let canvas_height = (frames.len() as f64 / cols as f64).ceil() as u32 * max_frame_height;

        let mut buffer = draw_frame_to_raw_buffer(&frames[0], palette, canvas_width, canvas_height, args.use_transparency);
        for (i, frame) in frames.iter().enumerate() {
            let col = (i as u32) % cols;
            let row = (i as u32) / cols;
            let base_x = col * max_frame_width;
            let base_y = row * max_frame_height;
            let temp_img = draw_frame_to_raw_buffer(frame, palette, max_frame_width, max_frame_height, args.use_transparency);

            for y in 0..max_frame_height {
                for x in 0..max_frame_width {
                    let dst_index = ((base_y + y) * canvas_width + (base_x + x)) as usize * if args.use_transparency { 4 } else { 3 };
                    let src_index = (y * max_frame_width + x) as usize * if args.use_transparency { 4 } else { 3 };
                    buffer[dst_index..dst_index + if args.use_transparency { 4 } else { 3 }]
                        .copy_from_slice(&temp_img[src_index..src_index + if args.use_transparency { 4 } else { 3 }]);
                }
            }
        }

        let image = if args.use_transparency {
            DynamicImage::ImageRgba8(
                ImageBuffer::from_raw(canvas_width, canvas_height, buffer)
                    .expect("Failed to create RGBA image"),
            )
        } else {
            DynamicImage::ImageRgb8(
                ImageBuffer::from_raw(canvas_width, canvas_height, buffer)
                    .expect("Failed to create RGB image"),
            )
        };

        let output_path = format!("{}/all_frames.png", args.output_path);
        image.save(&output_path).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        log(LogLevel::Info, &format!("Saved all frames to {}", output_path));

    } else {
        // Non-tiled mode, so we save each frame as a separate image.
        for (i, frame) in frames.iter().enumerate() {
            let buffer = draw_frame_to_raw_buffer(frame, palette, max_frame_width, max_frame_height, args.use_transparency);

            let image = if args.use_transparency {
                DynamicImage::ImageRgba8(
                    ImageBuffer::from_raw(max_frame_width, max_frame_height, buffer)
                        .expect("Failed to create RGBA image"),
                )
            } else {
                DynamicImage::ImageRgb8(
                    ImageBuffer::from_raw(max_frame_width, max_frame_height, buffer)
                        .expect("Failed to create RGB image"),
                )
            };

            let output_path = format!("{}/frame_{:03}.png", args.output_path, i);
            image.save(&output_path).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            log(LogLevel::Info, &format!("Saved frame {} to {}", i, output_path));
        }
    }

    Ok(())
}

fn map_color_to_palette_index(color: [u8; 3], palette: &[[u8; 3]]) -> u8 {
    let mut best_index = 0;
    let mut best_distance = u32::MAX;

    for (i, &pal_color) in palette.iter().enumerate() {
        let dr = color[0] as i32 - pal_color[0] as i32;
        let dg = color[1] as i32 - pal_color[1] as i32;
        let db = color[2] as i32 - pal_color[2] as i32;
        let dist = (dr * dr + dg * dg + db * db) as u32;

        if dist < best_distance {
            best_distance = dist;
            best_index = i;
        }
    }

    if best_distance != 0 {
        log(LogLevel::Info, &format!(
            "Non-exact color match for pixel [{}, {}, {}] — using palette index {} (distance = {})",
            color[0], color[1], color[2], best_index, best_distance
        ));
    }

    best_index as u8
}

pub fn png_to_pixels(png_file_name: &str, palette: &[[u8; 3]]) -> std::io::Result<(u8, u8, Vec<u8>)> {
    let img = image::open(png_file_name)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?
        .to_rgb8();
    let (width, height) = img.dimensions();
    log(LogLevel::Debug, &format!("Reading image {}. Dimensions: {}x{}", png_file_name, width, height));

    let mut pixels = Vec::with_capacity((width * height) as usize);
    for pixel in img.pixels() {
        let rgb = [pixel[0], pixel[1], pixel[2]];
        let index = map_color_to_palette_index(rgb, palette);
        pixels.push(index);
    }
    Ok((width as u8, height as u8, pixels))
}
