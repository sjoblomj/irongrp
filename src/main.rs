use std::fs::File;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::io::Result;
use image::{ImageBuffer, Rgba};
use clap::Parser;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the GRP file
    grp_path: String,

    /// Path to the PAL file
    pal_path: String,

    /// Output directory
    output_dir: String,
}

// GRP File Structure
#[derive(Debug)]
struct GrpHeader {
    frame_count: u16,
    max_width: u16,
    max_height: u16,
}

#[derive(Debug)]
struct GrpFrame {
    x_offset: u8,
    y_offset: u8,
    width: u8,
    height: u8,
    image_data_offset: u32,
}

/// Reads a PAL file (StarCraft palette format)
fn read_palette(pal_path: &str) -> Result<Vec<[u8; 3]>> {
    let mut file = File::open(pal_path)?;
    let mut buffer = [0u8; 768]; // PAL files contain 256 RGB entries (256 * 3 bytes = 768)
    file.read_exact(&mut buffer)?;

    Ok(buffer.chunks(3).map(|c| [c[0], c[1], c[2]]).collect())
}

fn read_grp_header(file: &mut File) -> Result<GrpHeader> {
    let mut buf = [0u8; 6];
    file.read_exact(&mut buf)?;
    let header = GrpHeader {
        frame_count: u16::from_le_bytes([buf[0], buf[1]]),
        max_width:   u16::from_le_bytes([buf[2], buf[3]]),
        max_height:  u16::from_le_bytes([buf[4], buf[5]]),
    };

    Ok(header)
}

// Reads all GRP frame headers
fn read_grp_frames(file: &mut File, frame_count: usize) -> Result<Vec<GrpFrame>> {
    let mut frames = Vec::new();
    for _ in 0..frame_count {
        let mut buf = [0u8; 8];
        file.read_exact(&mut buf)?;

        frames.push(GrpFrame {
            x_offset: buf[0],
            y_offset: buf[1],
            width:    buf[2],
            height:   buf[3],
            image_data_offset: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        });
    }
    Ok(frames)
}

// Reads row offsets and decodes image data
fn read_image_data(file: &mut File, frame: &GrpFrame) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(frame.image_data_offset as u64))?;

    let mut row_offsets = Vec::with_capacity(frame.height as usize);
    for _ in 0..frame.height {
        let mut row_offset_buf = [0u8; 2];
        file.read_exact(&mut row_offset_buf)?;
        let row_offset = u16::from_le_bytes(row_offset_buf) as u64;
        row_offsets.push(row_offset);
    }

    let mut pixels = vec![0; (frame.width as usize) * (frame.height as usize)];

    for (row, &row_offset) in row_offsets.iter().enumerate() {
        file.seek(SeekFrom::Start(frame.image_data_offset as u64 + row_offset))?;

        let mut row_data = Vec::new();
        file.read_to_end(&mut row_data)?;

        let decoded_row = decode_grp_rle_row(&row_data, frame.width as usize);
        let start = row * frame.width as usize;
        pixels[start..start + decoded_row.len()].copy_from_slice(&decoded_row);
    }

    Ok(pixels)
}

// Decodes an RLE-compressed row of pixels
fn decode_grp_rle_row(line_data: &[u8], image_width: usize) -> Vec<u8> {
    let mut line_pixels = vec![0; image_width]; // Initialize with transparent pixels (palette index 0)
    let mut x = 0; // Position in output row
    let mut data_offset = 0; // Position in input data

    while x < image_width && data_offset < line_data.len() {
        let control_byte = line_data[data_offset];
        data_offset += 1;

        if control_byte & 0x80 != 0 { // Transparent - skip x pixels
            let skip = (control_byte & 0x7F) as usize;
            x += skip;

        } else if control_byte & 0x40 != 0 { // Run-length encoding (repeat same color X times) 
            let run_length  = (control_byte & 0x3F) as usize;
            if data_offset >= line_data.len() { break; } // Safety check
            let color_index = line_data[data_offset];    // Color index from palette
            data_offset    += 1;

            for _ in 0..run_length {
                if x >= image_width { break; }
                line_pixels[x] = color_index;
                x += 1;
            }

        } else { // Normal - copy x pixels directly
            let copy_length = control_byte as usize;
            for _ in 0..copy_length {
                if x >= image_width || data_offset >= line_data.len() { break; }
                line_pixels[x] = line_data[data_offset];
                x += 1;
                data_offset += 1;
            }
        }
    }

    line_pixels
}

fn save_frame_as_png(
    frame: &GrpFrame,
    pixels: &[u8],
    palette: &[[u8; 3]],
    output_path: &str,
    max_width: u32,
    max_height: u32,
) -> Result<()> {

    let mut img = ImageBuffer::from_pixel(max_width as u32, max_height as u32, Rgba([0, 0, 0, 0]));

    let x_offset = frame.x_offset as u32;
    let y_offset = frame.y_offset as u32;

    for y in 0..frame.height as u32 {
        for x in 0..frame.width as u32 {
            let index = pixels[(y * frame.width as u32 + x) as usize] as usize;
            let color = palette[index];

            img.put_pixel(x + x_offset, y + y_offset, Rgba([color[0], color[1], color[2], 255]));
        }
    }

    img.save(output_path).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Ensure output directory exists
    fs::create_dir_all(&args.output_dir)?;

    // Load palette
    let palette = read_palette(&args.pal_path)?;

    // Load GRP file
    let mut file = File::open(&args.grp_path)?;

    // Read header and frames
    let header = read_grp_header(&mut file)?;
    let frames = read_grp_frames(&mut file, header.frame_count as usize)?;

    for (i, frame) in frames.iter().enumerate() {
        let pixels = read_image_data(&mut file, frame)?;

        let output_file = format!("{}/frame_{:03}.png", args.output_dir, i);
        save_frame_as_png(frame, &pixels, &palette, &output_file, header.max_width as u32, header.max_height as u32)?;

        println!("Saved: {}", output_file);
    }

    println!("Conversion complete!");
    Ok(())
}
