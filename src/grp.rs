use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Result, Write};
use crate::{Args, LogLevel, log, list_png_files};
use crate::png::{png_to_pixels, render_and_save_frames_to_png};

#[derive(Debug)]
struct GrpHeader {
    frame_count: u16,
    max_width:   u16,
    max_height:  u16,
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
    pub row_offsets:  Vec<u16>,
    /// List of rows of raw image data
    pub raw_row_data: Vec<Vec<u8>>,
    /// The raw image data, converted to pixels
    pub converted_pixels: Vec<u8>,
}

impl GrpFrame {
    /// The length of the frame in bytes, as it would be written to a GRP file
    fn grp_frame_len(&self) -> usize {
        let frame_header_size    = std::mem::size_of::<u8>() * 4 + std::mem::size_of::<u32>();
        let row_offsets_size     = self.image_data.row_offsets.len() * 2; // u16 = 2 bytes
        let raw_data_size: usize = self.image_data.raw_row_data.iter().map(|row| row.len()).sum();
        frame_header_size + row_offsets_size + raw_data_size
    }
}

/// Reads a PAL file (StarCraft palette format)
fn read_palette(pal_path: &str) -> Result<Vec<[u8; 3]>> {
    let mut file = File::open(pal_path)?;
    let mut buffer = [0u8; 768]; // PAL files contain 256 RGB entries (256 * 3 bytes = 768)
    file.read_exact(&mut buffer)?;

    Ok(buffer.chunks(3).map(|c| [c[0], c[1], c[2]]).collect())
}

/// Parses the header of a GRP file
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

/// Parses all GRP frames
fn read_grp_frames(file: &mut File, frame_count: usize) -> Result<Vec<GrpFrame>> {
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

/// Reads row offsets and decodes image data
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

/// Decodes an RLE-compressed row of pixels
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

/// Encodes pixels to an RLE-compressed ImageData
fn encode_grp_rle_data(width: u8, height: u8, pixels: Vec<u8>) -> ImageData {
    let mut raw_row_data = Vec::new();
    let mut rle_data     = Vec::new();
    let mut row_offsets  = Vec::with_capacity(height as usize);

    for row in 0..height {
        let row_start_offset = rle_data.len();
        row_offsets.push(row_start_offset as u16);

        let start = row as usize * width as usize;
        let end = start + width as usize;
        let row_pixels = &pixels[start..end];

        let mut x = 0;
        while x < row_pixels.len() {
            let current = row_pixels[x];

            // Case 1: Transparent run (index 0)
            if current == 0 {
                let mut run = 1;
                while x + run < row_pixels.len() && row_pixels[x + run] == 0 && run < 127 {
                    run += 1;
                }
                rle_data.push(0x80 | run as u8);
                x += run;
            }

            // Case 2: Run of the same color (but not transparent)
            else {
                let mut run = 1;
                while x + run < row_pixels.len()
                    && row_pixels[x + run] == current
                    && run < 63
                {
                    run += 1;
                }

                if run >= 3 {
                    rle_data.push(0x40 | run as u8);
                    rle_data.push(current);
                    x += run;
                } else {
                    // Case 3: Literal copy
                    let mut run = 1;
                    while x + run < row_pixels.len()
                        && (row_pixels[x + run] != 0
                        && (row_pixels[x + run] != row_pixels[x + run - 1] || run < 3))
                        && run < 63
                    {
                        run += 1;
                    }

                    rle_data.push(run as u8);
                    for i in 0..run {
                        rle_data.push(row_pixels[x + i]);
                    }
                    x += run;
                }
            }
        }
        raw_row_data.push(rle_data[row_start_offset..].to_vec());
    }

    ImageData {
       row_offsets,
       raw_row_data,
       converted_pixels: pixels,
    }
}

/// Creates a GrpHeader from a set of GrpFrames
fn create_grp_header(frames: &[GrpFrame]) -> GrpHeader {
    let max_width = frames
        .iter()
        .map(|f| f.width as u16 + f.x_offset as u16)
        .max()
        .unwrap_or(0);

    let max_height = frames
        .iter()
        .map(|f| f.height as u16 + f.y_offset as u16)
        .max()
        .unwrap_or(0);

    GrpHeader {
        frame_count: frames.len() as u16,
        max_width,
        max_height,
    }
}


/// Given a path, GrpHeader and a set of GrpFrames, this function writes a GRP file
/// to the given path.
fn write_grp_file(path: &str, header: &GrpHeader, frames: &[GrpFrame]) -> Result<()> {
    let mut file = File::create(path)?;

    // Write header
    file.write_all(&header.frame_count.to_le_bytes())?;
    file.write_all(&header.max_width  .to_le_bytes())?;
    file.write_all(&header.max_height .to_le_bytes())?;

    // Write frame headers
    for frame in frames {
        file.write_all(&[frame.x_offset])?;
        file.write_all(&[frame.y_offset])?;
        file.write_all(&[frame.width])?;
        file.write_all(&[frame.height])?;
        file.write_all(&frame.image_data_offset.to_le_bytes())?;
    }

    // Write image data
    for frame in frames {
        // Write row offset table
        for &offset in &frame.image_data.row_offsets {
            file.write_all(&offset.to_le_bytes())?;
        }

        // Write each row's raw RLE data
        for row in &frame.image_data.raw_row_data {
            file.write_all(row)?;
        }
    }

    Ok(())
}

/// Read the PNG in the given file name, and turn it into a GrpFrame
fn png_to_grpframe(png_file_name: &str, palette: &[[u8; 3]], image_data_offset: u32) -> std::io::Result<GrpFrame> {
    let (width, height, pixels) = png_to_pixels(png_file_name, palette)?;
    let image_data = encode_grp_rle_data(width as u8, height as u8, pixels);

    Ok(GrpFrame {
        x_offset: 0, // configurable if needed
        y_offset: 0,
        width:  width  as u8,
        height: height as u8,
        image_data_offset,
        image_data,
    })
}

/// Turn all the given PNG files into a set of GrpFrames.
fn files_to_grp(png_files: Vec<String>, palette: &[[u8; 3]]) -> std::io::Result<Vec<GrpFrame>> {
    let mut image_data_offset = (6 + png_files.len() * 8) as u32; // Initialize to GRP header size
    let mut grp_frames = Vec::with_capacity(png_files.len());

    for png_file in png_files {
        let grp_frame = png_to_grpframe(png_file.as_str(), palette, image_data_offset)?;
        image_data_offset += grp_frame.grp_frame_len() as u32;
        grp_frames.push(grp_frame);
    }
    Ok(grp_frames)
}


/// Converts a GRP to PNGs
pub fn grp_to_png(args: &Args) -> std::io::Result<()> {
    let palette = read_palette(&args.pal_path)?;

    let mut f  = std::fs::File::open(&args.input_path)?;
    let header = read_grp_header(&mut f)?;
    let frames = read_grp_frames(&mut f, header.frame_count as usize)?;

    render_and_save_frames_to_png(
        &frames,
        &palette,
        header.max_width  as u32,
        header.max_height as u32,
        &args,
    )
}

/// Converts PNGs to a GRP
pub fn png_to_grp(args: &Args) -> std::io::Result<()> {
    let palette    = read_palette(&args.pal_path)?;
    let png_files  = list_png_files(&args.input_path)?;
    let grp_frames = files_to_grp(png_files, &palette)?;
    let grp_header = create_grp_header(&grp_frames);
    write_grp_file(&args.output_path, &grp_header, &grp_frames)
}
