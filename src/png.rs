use crate::{GrpFrame, LogLevel, log, Args};
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
            let palette_index = frame.pixels[idx] as usize;
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
        let mut cols = ((frames.len() as f64).sqrt()).floor() as u32;
        log(LogLevel::Debug, &format!(
            "Saving all frames as one PNG. Columns: {}, max-frame-size: {}x{}, requested max width: {}",
            cols,
            max_frame_width,
            max_frame_height,
            args.max_width.unwrap_or(0)
        ));

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

        let output_path = format!("{}/all_frames.png", args.output_dir);
        image.save(&output_path).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        log(LogLevel::Info, &format!("Saved all frames to {}", output_path));
    } else {
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

            let output_path = format!("{}/frame_{:03}.png", args.output_dir, i);
            image.save(&output_path).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            log(LogLevel::Info, &format!("Saved frame {} to {}", i, output_path));
        }
    }

    Ok(())
}
