use std::fs::File;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::io::Result;
use image::{ImageBuffer, Rgba};
use clap::{Parser, ValueEnum};
use std::fmt;
use std::sync::OnceLock;

static LOG_LEVEL: OnceLock<LogLevel> = OnceLock::new();

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the GRP file
    grp_path: String,

    /// Path to the PAL file
    pal_path: String,

    /// Output directory
    output_dir: String,

    /// Logging level (debug, info, error)
    #[arg(long, value_enum, default_value_t = LogLevel::Info)]
    log_level: LogLevel,
}

#[derive(Clone, ValueEnum, Debug)]
enum LogLevel {
    Debug,
    Info,
    Error,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}


fn log(level: LogLevel, message: &str) {
    let level_order = |lvl: &LogLevel| match lvl {
        LogLevel::Debug => 0,
        LogLevel::Info => 1,
        LogLevel::Error => 2,
    };

    if let Some(current_level) = LOG_LEVEL.get() {
        if level_order(&level) >= level_order(current_level) {
            println!("[{level}] {message}");
        }
    }
}


// GRP File Structure
#[derive(Debug)]
struct GrpHeader {
    frame_count: u16,
    max_width: u16,
    max_height: u16,
}

#[derive(Clone, Copy, Debug)]
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

    log(LogLevel::Debug, &format!("Read GRP Header. Frame count: {}, max width: {}, max_height: {}", header.frame_count, header.max_width, header.max_height));
    Ok(header)
}

// Reads all GRP frame headers
fn read_grp_frames(file: &mut File, frame_count: usize) -> Result<Vec<GrpFrame>> {
    let mut frames = Vec::new();
    for i in 0..frame_count {
        let mut buf = [0u8; 8];
        file.read_exact(&mut buf)?;

        let grp_frame = GrpFrame {
            x_offset: buf[0],
            y_offset: buf[1],
            width:    buf[2],
            height:   buf[3],
            image_data_offset: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        };
        frames.push(grp_frame);
        log(LogLevel::Debug, &format!("Read GRP Frame {}. x-offset: {}, y-offset: {}, width: {}, height: {}, image-data-offset: {}", i, grp_frame.x_offset, grp_frame.y_offset, grp_frame.width, grp_frame.height, grp_frame.image_data_offset));
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
        log(LogLevel::Debug, &format!("Reading frame row of width {} from offset {}", frame.width, frame.image_data_offset as u64 + row_offset));

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
            log(LogLevel::Debug, &format!("Transparent byte. Skipping {} pixels.", skip));

        } else if control_byte & 0x40 != 0 { // Run-length encoding (repeat same color X times) 
            let run_length  = (control_byte & 0x3F) as usize;
            if data_offset >= line_data.len() { // Safety check
                log(LogLevel::Error, &format!("Run-length encoding error: Requested offset ({}) is greater than line length ({}).", data_offset, line_data.len()));
                break;
            }
            let color_index = line_data[data_offset]; // Color index from palette
            data_offset    += 1;
            log(LogLevel::Debug, &format!("Run-length encoding. Pixel with palette index {} will be repeated {} times.", color_index, run_length));

            for _ in 0..run_length {
                if x >= image_width {
                    log(LogLevel::Error, &format!("Run-length encoding error: X position ({}) is greater than image width ({}).", x, image_width));
                    break;
                }
                line_pixels[x] = color_index;
                x += 1;
            }

        } else { // Normal - copy x pixels directly
            let copy_length = control_byte as usize;
            log(LogLevel::Debug, &format!("Normal encoding. Will copy {} pixels.", copy_length));
            for _ in 0..copy_length {
                if x >= image_width || data_offset >= line_data.len() {
                    log(LogLevel::Error, &format!("Encoding error: X position ({}) is greater than image width ({}), or data offset ({}) is greater than line length ({}).", x, image_width, data_offset, line_data.len()));
                    break;
                }
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
    // Set the global log level
    LOG_LEVEL.set(args.log_level.clone()).expect("Failed to set log level");

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

        log(LogLevel::Info, &format!("Saved frame {} to file {}", i, output_file));
    }

    log(LogLevel::Info, "Conversion complete");
    Ok(())
}
