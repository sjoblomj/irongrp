use std::fmt;
use std::fs;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Result};
use std::sync::OnceLock;
use clap::{Parser, ValueEnum};

pub mod png;

pub static LOG_LEVEL: OnceLock<LogLevel> = OnceLock::new();

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Path to the GRP file, or directory containing PNG files
    pub input_path: String,

    /// Path to the PAL file
    pub pal_path: String,

    /// Output directory
    pub output_dir: String,

    /// Mode of operation (grp2png, png2grp)
    #[arg(long, value_enum, default_value_t = OperationMode::Grp2Png)]
    pub mode: OperationMode,

    /// Output all frames in one image. GRPs cannot be
    /// created back from tiled images.
    #[arg(long)]
    pub tiled: bool,

    /// Only applicable when using the 'tiled' argument.
    /// Maximum width of the output tiled image.
    /// If this is less than the maximum frame width of
    /// the GRP itself, this value will be ignored.
    #[arg(long)]
    pub max_width: Option<u32>,

    /// Enable transparency in the PNG images. Default
    /// behavior is to use index 0 in the palette.
    #[arg(long)]
    pub use_transparency: bool,

    /// Logging level (debug, info, error)
    #[arg(long, value_enum, default_value_t = LogLevel::Info)]
    pub log_level: LogLevel,
}

#[derive(Clone, ValueEnum, PartialEq)]
pub enum OperationMode {
    Grp2Png,
    Png2Grp,
}

#[derive(Clone, ValueEnum, Debug)]
pub enum LogLevel {
    Debug,
    Info,
    Error,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}


pub fn log(level: LogLevel, message: &str) {
    let level_order = |lvl: &LogLevel| match lvl {
        LogLevel::Debug => 0,
        LogLevel::Info  => 1,
        LogLevel::Error => 2,
    };

    if let Some(current_level) = LOG_LEVEL.get() {
        if level_order(&level) >= level_order(current_level) {
            println!("[{level}] {message}");
        }
    }
}


pub fn list_png_files(dir: &str) -> Result<Vec<String>> {
    let mut entries: Vec<_> = fs::read_dir(dir)?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()?.to_str()?.eq_ignore_ascii_case("png") {
                path.to_str().map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();

    entries.sort();
    Ok(entries)
}


// GRP File Structure
#[derive(Debug)]
pub struct GrpHeader {
    pub frame_count: u16,
    pub max_width:   u16,
    pub max_height:  u16,
}

#[derive(Clone, Debug)]
pub struct GrpFrame {
    pub x_offset: u8,
    pub y_offset: u8,
    pub width:    u8,
    pub height:   u8,
    pub image_data_offset: u32,
    pub image_data: ImageData,
}

#[derive(Clone, Debug)]
pub struct ImageData {
    /// offsets to the rows of raw data, relative to the image_data_offset
    pub row_offsets:      Vec<u16>,
    /// List of rows of raw image data
    pub raw_row_data:     Vec<Vec<u8>>,
    /// The raw image data, converted to pixels
    pub converted_pixels: Vec<u8>,
}

/// Reads a PAL file (StarCraft palette format)
pub fn read_palette(pal_path: &str) -> Result<Vec<[u8; 3]>> {
    let mut file = File::open(pal_path)?;
    let mut buffer = [0u8; 768]; // PAL files contain 256 RGB entries (256 * 3 bytes = 768)
    file.read_exact(&mut buffer)?;

    Ok(buffer.chunks(3).map(|c| [c[0], c[1], c[2]]).collect())
}

pub fn read_grp_header(file: &mut File) -> Result<GrpHeader> {
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
pub fn read_grp_frames(file: &mut File, frame_count: usize) -> Result<Vec<GrpFrame>> {
    let pos = file.stream_position()?;
    let mut frames = Vec::new();
    for i in 0..frame_count {
        file.seek(SeekFrom::Start(pos + (i * 8) as u64))?;
        let mut buf = [0u8; 8];
        file.read_exact(&mut buf)?;

        let image_data_offset = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let image_data = read_image_data(file, buf[2] as usize, buf[3] as usize, image_data_offset as u64)?;
        let grp_frame = GrpFrame {
            x_offset: buf[0],
            y_offset: buf[1],
            width:    buf[2],
            height:   buf[3],
            image_data_offset: image_data_offset,
            image_data,
        };
        frames.push(grp_frame.clone());
        log(LogLevel::Debug, &format!("Read GRP Frame {}. x-offset: {}, y-offset: {}, width: {}, height: {}, image-data-offset: {}, number of pixels: {}", i, grp_frame.x_offset, grp_frame.y_offset, grp_frame.width, grp_frame.height, grp_frame.image_data_offset, grp_frame.image_data.converted_pixels.len()));
    }
    Ok(frames)
}

// Reads row offsets and decodes image data
fn read_image_data(
    file: &mut File,
    width: usize,
    height: usize,
    image_data_offset: u64,
) -> Result<ImageData> {
    file.seek(SeekFrom::Start(image_data_offset))?;

    let mut row_offsets = Vec::with_capacity(height);
    for _ in 0..height {
        let mut row_offset_buf = [0u8; 2];
        file.read_exact(&mut row_offset_buf)?;
        let row_offset = u16::from_le_bytes(row_offset_buf);
        row_offsets.push(row_offset);
    }

    let mut raw_row_data = Vec::with_capacity(height);
    let mut pixels = vec![0; width * height];

    for (row, &row_offset) in row_offsets.iter().enumerate() {
        file.seek(SeekFrom::Start(image_data_offset + row_offset as u64))?;
        log(LogLevel::Debug, &format!("Reading frame row of width {} from offset {}", width, image_data_offset + row_offset as u64));

        let mut row_data = Vec::new();
        file.read_to_end(&mut row_data)?;
        raw_row_data.push(row_data.clone());

        let decoded_row = decode_grp_rle_row(&row_data, width);
        let start = row * width;
        pixels[start..start + decoded_row.len()].copy_from_slice(&decoded_row);
    }

    Ok(ImageData {
        row_offsets,
        raw_row_data,
        converted_pixels: pixels,
    })
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
            data_offset += 1;
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
