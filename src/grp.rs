use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Result, Write};
use crate::{Args, CompressionType, LogLevel, log, list_png_files};
use crate::png::{png_to_pixels, render_and_save_frames_to_png};

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
    pub row_offsets:  Vec<u16>,
    /// List of rows of raw image data
    pub raw_row_data: Vec<Vec<u8>>,
    /// The raw image data, converted to pixels
    pub converted_pixels: Vec<u8>,
}

impl GrpFrame {
    /// The length of the frame in bytes, as it would be written to a GRP file
    fn grp_frame_len(&self) -> usize {
        let row_offsets_size     = self.image_data.row_offsets.len() * 2; // u16 = 2 bytes
        let raw_data_size: usize = self.image_data.raw_row_data.iter().map(|row| row.len()).sum();
        row_offsets_size + raw_data_size
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
pub fn read_grp_header<R: Read + Seek>(file: &mut R) -> Result<GrpHeader> {
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
pub fn read_grp_frames<R: Read + Seek>(file: &mut R, frame_count: usize) -> Result<Vec<GrpFrame>> {
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
fn read_image_data<R: Read + Seek>(
    file:   &mut R,
    width:  usize,
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
        let row_pos  = image_data_offset + row_offset as u64;
        let file_len = file.seek(SeekFrom::End(0))?;
        if row_pos >= file_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("Row offset {} is beyond file length {}", row_pos, file_len),
            ));
        }
        file.seek(SeekFrom::Start(row_pos))?;
        log(LogLevel::Debug, &format!("Reading frame row of width {} from offset {}", width, image_data_offset + row_offset as u64));

        let mut row_data = vec![0; width];
        file.read(&mut row_data)?;
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


/// Encodes an RLE-compressed row of pixels
fn encode_grp_rle_row(row_pixels: &[u8], compression_type: &CompressionType) -> Vec<u8> {
    let mut encoded = Vec::new();
    let mut i = 0;
    for x in 0..row_pixels.len() {
        println!("x: {:2}, row_pixels[i]: {:3X}", x, row_pixels[x]);
    }

    let same_colour_threshold = if let CompressionType::Blizzard = compression_type {
        4
    } else {
        2
    };

    let mut safety_break = 0;
    while i < row_pixels.len() {
        safety_break += 1;
        if safety_break > 4096 {
            log(LogLevel::Error, "Seems like we're stuck in an infinite loop, after 4096 iterations. Breaking.");
            break;
        }
        let current_colour = row_pixels[i];

        println!("Encoding pixel at position {} / {} with palette index {}", i, row_pixels.len(), current_colour);
        // Case 1: Transparent run (index 0)
        if current_colour == 0 {
            let mut run_len = 1;
            while i + run_len < row_pixels.len() && row_pixels[i + run_len] == 0 && run_len < 127 {
                run_len += 1;
            }
            println!("Transparent run of {} ({:X}) - {}", run_len, run_len, 0x80 | run_len as u8);
            encoded.push(0x80 | run_len as u8);
            i += run_len;

        } else { // Case 2: Run of the same color (but not transparent)
            let mut run_len = 1;
            while i + run_len < row_pixels.len()
                && row_pixels[i + run_len] == current_colour
                && run_len < 63
            {
                run_len += 1;
            }
            println!("Pixels of the same colour: {} ({:X})", run_len, run_len);

            if run_len > same_colour_threshold {
                println!(">=3. Same colour {} ({:X}) - {} {:3}", run_len, run_len, 0x40 | run_len as u8, current_colour);
                encoded.push(0x40 | run_len as u8);
                encoded.push(current_colour);
                i += run_len;

            } else { // Case 3: Literal copy
                let start = i;
                let mut run_len = 0;
                let mut last_colour = 0;
                let mut last_colour_len = 0;

                // Go through the row until we find a run of same coloured pixels above the threshold
                for x in i..row_pixels.len() {
                    println!("xx: {:2}, row_pixels[i]: {:3X} ({:3})", x, row_pixels[x], row_pixels[x]);
                    if row_pixels[x] == 0 {
                        break;
                    }
                    if row_pixels[x] != last_colour || last_colour_len == 0 {
                        // New pixel or first pixel
                        last_colour = row_pixels[x];
                        last_colour_len = 1;
                    } else {
                        // Repetition of last seen pixel
                        last_colour_len += 1;
                    }

                    if last_colour_len > same_colour_threshold {
                        run_len -= same_colour_threshold;
                        break;
                    }
                    if run_len >= 63 {
                        break;
                    }
                    run_len += 1;
                }

                //while i < row_pixels.len()
                //    && (run_len == 0 || row_pixels[i] != row_pixels[i + 1])
                //    && row_pixels[i] != 0
                //    && run_len < 63
                //{
                //    if i + 1 < row_pixels.len() {
                //        println!("i: {:2}, run_len: {:2}, row_pixels[i]: {:3X}, row_pixels[i + 1]: {:3X}", i, run_len, row_pixels[i], row_pixels[i + 1]);
                //    }
                //    //println!("run_len: {:2}, row_pixels[i + run_len]: {:3X}, row_pixels[i + run_len - 1]: {:3X}", run_len, row_pixels[i + run_len], row_pixels[i + run_len - 1]);
                //    run_len += 1;
                //    i += 1;
                //}
                println!("Literal copy {} ({:X})", run_len, run_len);

                encoded.push(run_len as u8);
                encoded.extend_from_slice(&row_pixels[start..start + run_len]);
                i += run_len;
            }
        }
    }

    encoded
}


/// Encodes pixels to an RLE-compressed ImageData
fn encode_grp_rle_data(width: u8, height: u8, pixels: Vec<u8>, compression_type: &CompressionType) -> ImageData {
    let mut raw_row_data = Vec::new();
    let mut rle_data     = Vec::new();
    let mut row_offsets  = Vec::with_capacity(height as usize);

    for row in 0..height {
        let row_start_offset = rle_data.len() + (height * 2) as usize;
        row_offsets.push(row_start_offset as u16);

        let start = row as usize * width as usize;
        let end = start + width as usize;
        let row_pixels = &pixels[start..end];

        let encoded_row = encode_grp_rle_row(row_pixels, compression_type);
        rle_data.extend_from_slice(&encoded_row);
        raw_row_data.push(encoded_row);
    }

    ImageData {
       row_offsets,
       raw_row_data,
       converted_pixels: pixels,
    }
}

fn get_max_size(frames: &[GrpFrame], dimension: fn(&GrpFrame) -> u8) -> u16 {
    return frames
        .iter()
        .map(|f| dimension(f) as u16)
        .max()
        .unwrap_or(0);
}

/// Creates a GrpHeader from a set of GrpFrames
fn create_grp_header(frames: &[GrpFrame]) -> GrpHeader {
    let max_width  = get_max_size(frames, |f| f.width  + (f.x_offset * 2));
    let max_height = get_max_size(frames, |f| f.height + (f.y_offset * 2));

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
fn png_to_grpframe(png_file_name: &str, palette: &[[u8; 3]], image_data_offset: u32, compression_type: &CompressionType) -> std::io::Result<GrpFrame> {
    let image = png_to_pixels(png_file_name, palette)?;
    let image_data = encode_grp_rle_data(image.width, image.height, image.image_data, compression_type);

    Ok(GrpFrame {
        x_offset: image.x_offset,
        y_offset: image.y_offset,
        width:  image.width,
        height: image.height,
        image_data_offset,
        image_data,
    })
}

/// Turn all the given PNG files into a set of GrpFrames.
fn files_to_grp(png_files: Vec<String>, palette: &[[u8; 3]], compression_type: &CompressionType) -> std::io::Result<Vec<GrpFrame>> {
    let mut image_data_offset = (6 + png_files.len() * 8) as u32; // Initialize to GRP header size
    let mut grp_frames = Vec::with_capacity(png_files.len());

    for png_file in png_files {
        let grp_frame = png_to_grpframe(png_file.as_str(), palette, image_data_offset, compression_type)?;
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
    let grp_frames = files_to_grp(png_files, &palette, &args.compression_type)?;
    let grp_header = create_grp_header(&grp_frames);
    write_grp_file(&args.output_path, &grp_header, &grp_frames)
}


#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use super::*;

    #[test]
    fn test_malformed_header() {
        use std::io::Cursor;
        let data = vec![0u8; 4]; // too short for a valid header
        let mut cursor = Cursor::new(data);

        let result = read_grp_header(&mut cursor);

        assert!(result.is_err());
    }

    #[test]
    fn test_incomplete_frame_header() {
        use std::io::Cursor;
        let mut data = vec![0x01, 0x00, 0x01, 0x00, 0x01, 0x00]; // 1 frame, 1x1 size
        data.extend(vec![0; 4]); // only half a frame header
        let mut cursor = Cursor::new(data);

        let _ = read_grp_header(&mut cursor); // skip header
        let result = read_grp_frames(&mut cursor, 1);

        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_row_offset() {
        use std::io::Cursor;
        // Valid header + 1 frame header
        let mut data = vec![0x01, 0x00, 0x01, 0x00, 0x01, 0x00]; // 1 frame, 1x1 size
        data.extend(vec![0, 0, 1, 1, 14, 0, 0, 0]); // frame header (offset 14)
        data.extend(vec![0xFF, 0xFF]); // row offset points far beyond file

        let mut cursor = Cursor::new(data);
        let _ = read_grp_header(&mut cursor);
        let result = read_grp_frames(&mut cursor, 1);
        assert!(result.is_err());
    }


    #[test]
    fn test_decode_transparent_only() {
        let data = vec![0x85]; // skip 5 transparent pixels

        let result = decode_grp_rle_row(&data, 5);

        assert_eq!(result, vec![0, 0, 0, 0, 0]);
    }

    #[test]
    fn test_decode_solid_color_run() {
        let data = vec![0x42, 7]; // repeat color 7 for 2 pixels

        let result = decode_grp_rle_row(&data, 2);

        assert_eq!(result, vec![7, 7]);
    }

    #[test]
    fn test_decode_raw_pixels() {
        let data = vec![3, 5, 6, 7]; // copy 3 pixels directly

        let result = decode_grp_rle_row(&data, 3);

        assert_eq!(result, vec![5, 6, 7]);
    }

    #[test]
    fn test_decode_mixed_sequence() {
        let data = vec![0x81, 0x43, 9, 2, 8, 7];
        // skip 1 transparent, repeat 9 for 3, then copy 2 pixels (8, 7)

        let result = decode_grp_rle_row(&data, 6);

        assert_eq!(result, vec![0, 9, 9, 9, 8, 7]);
    }


    #[test]
    fn test_encode_transparent_only() {
        // A row with 5 transparent pixels (palette index 0)
        let row = vec![0; 5];

        let encoded_blizz = encode_grp_rle_row(&row, &CompressionType::Blizzard);
        let encoded_optim = encode_grp_rle_row(&row, &CompressionType::Optimised);

        // 0x80 means transparent run; 0x80 | 5 = 0x85
        assert_eq!(encoded_blizz, vec![0x85]);
        assert_eq!(encoded_optim, vec![0x85]);
    }

    #[test]
    fn test_encode_solid_color_run() {
        // A row with 4 pixels of the same color (e.g. 7)
        let row = vec![7; 4];

        let encoded_blizz = encode_grp_rle_row(&row, &CompressionType::Blizzard);
        let encoded_optim = encode_grp_rle_row(&row, &CompressionType::Optimised);

        // 0x40 means repeated color; 0x40 | 4 = 0x44, followed by the color
        assert_eq!(encoded_blizz, vec![0x04, 7, 7, 7, 7]);
        assert_eq!(encoded_optim, vec![0x44, 7]);
    }

    #[test]
    fn test_encode_raw_pixels() {
        // A row with 3 different pixels (no repetition)
        let row = vec![5, 6, 7];

        let encoded_blizz = encode_grp_rle_row(&row, &CompressionType::Blizzard);
        let encoded_optim = encode_grp_rle_row(&row, &CompressionType::Optimised);

        // No compression, just copy 3 pixels: [3, 5, 6, 7]
        assert_eq!(encoded_blizz, vec![0x03, 5, 6, 7]);
        assert_eq!(encoded_optim, vec![0x03, 5, 6, 7]);
    }

    #[test]
    fn test_encode_mixed_sequence() {
        // Mixed content:
        // 1 transparent pixel, 3 repeated 9s, and then 2 different pixels
        let row = vec![0, 9, 9, 9, 8, 7];

        let encoded_blizz = encode_grp_rle_row(&row, &CompressionType::Blizzard);
        let encoded_optim = encode_grp_rle_row(&row, &CompressionType::Optimised);

        // Breakdown:
        // - 0x81: skip 1 transparent
        // - 0x43, 9: repeat 9 for 3 times
        // - 0x02, 8, 7: copy 2 pixels
        assert_eq!(encoded_blizz, vec![0x81, 0x05, 9, 9, 9, 8, 7]);
        assert_eq!(encoded_optim, vec![0x81, 0x43, 9, 0x02, 8, 7]);
    }


    #[test]
    fn test_encode_max_transparent_run() {
        let row = vec![0; 127];

        let encoded_blizz = encode_grp_rle_row(&row, &CompressionType::Blizzard);
        let encoded_optim = encode_grp_rle_row(&row, &CompressionType::Optimised);

        assert_eq!(encoded_blizz, vec![0xFF]); // 0x80 | 127
        assert_eq!(encoded_optim, vec![0xFF]); // 0x80 | 127
    }

    #[test]
    fn test_encode_max_solid_color_run() {
        let row = vec![12; 63];

        let encoded_blizz = encode_grp_rle_row(&row, &CompressionType::Blizzard);
        let encoded_optim = encode_grp_rle_row(&row, &CompressionType::Optimised);

        assert_eq!(encoded_blizz, vec![0x7F, 12]); // 0x40 | 63 = 0x7F
        assert_eq!(encoded_optim, vec![0x7F, 12]); // 0x40 | 63 = 0x7F
    }

    #[test]
    fn test_encode_max_raw_copy() {
        let row: Vec<u8> = (1..63).collect();

        let encoded_blizz = encode_grp_rle_row(&row, &CompressionType::Blizzard);
        let encoded_optim = encode_grp_rle_row(&row, &CompressionType::Optimised);

        let mut expected = vec![62];
        expected.extend(row.iter());
        assert_eq!(encoded_blizz, expected);
        assert_eq!(encoded_optim, expected);
    }

    #[test]
    fn test_encode_alternating_transparency() {
        let row = vec![0, 1, 0, 2, 0, 3];

        let encoded_blizz = encode_grp_rle_row(&row, &CompressionType::Blizzard);
        let encoded_optim = encode_grp_rle_row(&row, &CompressionType::Optimised);

        // Should encode as a series of transparent skips and literal copies.
        // Before each literal copy there is a number (here 1 in each case)
        // denoting how many pixels of that copy.
        assert_eq!(encoded_blizz, vec![0x81, 0x01, 1, 0x81, 0x01, 2, 0x81, 0x01, 3]);
        assert_eq!(encoded_optim, vec![0x81, 0x01, 1, 0x81, 0x01, 2, 0x81, 0x01, 3]);
    }

    #[test]
    fn test_encode_then_decode_roundtrip_with_differences_between_compression_types() {
        let original = vec![0x8F, 0x02, 0x8A, 0x40, 0x48, 0x8B, 0x04, 0x40, 0x40, 0x40, 0x8A, 0x8F];
        let width = 44;

        let decoded = decode_grp_rle_row(&original, width);
        let encoded_blizz = encode_grp_rle_row(&decoded, &CompressionType::Blizzard);
        let encoded_optim = encode_grp_rle_row(&decoded, &CompressionType::Optimised);

        assert_eq!(encoded_blizz, original);
        assert_eq!(encoded_optim, vec![0x8F, 0x02, 138, 64, 0x48, 139, 0x43, 64, 0x01, 138, 0x8F]);
    }

    #[test]
    fn test_encode_then_decode_roundtrip() {
        let original = vec![0, 0, 7, 7, 7, 8, 9];
        let width = original.len();

        let encoded_blizz = encode_grp_rle_row(&original, &CompressionType::Blizzard);
        let encoded_optim = encode_grp_rle_row(&original, &CompressionType::Optimised);
        let decoded_blizz = decode_grp_rle_row(&encoded_blizz, width);
        let decoded_optim = decode_grp_rle_row(&encoded_optim, width);

        assert_eq!(original, decoded_blizz);
        assert_eq!(original, decoded_optim);
    }

    #[test]
    fn test_roundtrip_various_patterns() {
        let test_rows = vec![
            vec![0, 0, 0, 0, 0],
            vec![1, 2, 3, 4, 5],
            vec![5, 5, 5, 5, 5],
            vec![0, 1, 1, 1, 0, 2, 2],
            vec![1, 2, 2, 2, 3, 0, 0],
        ];

        for row in test_rows {
            let encoded_blizz = encode_grp_rle_row(&row, &CompressionType::Blizzard);
            let encoded_optim = encode_grp_rle_row(&row, &CompressionType::Optimised);
            let decoded_blizz = decode_grp_rle_row(&encoded_blizz, row.len());
            let decoded_optim = decode_grp_rle_row(&encoded_optim, row.len());
            assert_eq!(decoded_blizz, row);
            assert_eq!(decoded_optim, row);
        }
    }

    #[test]
    fn test_roundtrip_edge_cases() {
        let max_transparent = vec![0; 127];
        let max_solid_color = vec![42; 63];
        let max_raw_copy: Vec<u8> = (0..63).collect();
        let combo = [&[0; 3][..], &[5; 5][..], &[1, 2, 3][..]].concat();

        let edge_cases = vec![
            max_transparent,
            max_solid_color,
            max_raw_copy,
            combo,
        ];

        for row in edge_cases {
            let encoded_blizz = encode_grp_rle_row(&row, &CompressionType::Blizzard);
            let encoded_optim = encode_grp_rle_row(&row, &CompressionType::Optimised);
            let decoded_blizz = decode_grp_rle_row(&encoded_blizz, row.len());
            let decoded_optim = decode_grp_rle_row(&encoded_optim, row.len());
            assert_eq!(decoded_blizz, row);
            assert_eq!(decoded_optim, row);
        }
    }


    #[test]
    fn test_decode_truncated_run_length() {
        // Claims to repeat a color, but color byte is missing
        let data = vec![0x41]; // run-length of 1, but no color follows

        let result = decode_grp_rle_row(&data, 1);

        // Expect a fallback to default pixel value (0)
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn test_decode_run_exceeds_width() {
        // Claims to repeat 5 pixels but only room for 3
        let data = vec![0x45, 7]; // run-length of 5 with color 7

        let result = decode_grp_rle_row(&data, 3);

        // Should clamp at width
        assert_eq!(result, vec![7, 7, 7]);
    }

    #[test]
    fn test_decode_raw_exceeds_data() {
        // Claims to copy 3 pixels but only 2 are present
        let data = vec![3, 1, 2];

        let result = decode_grp_rle_row(&data, 3);

        assert_eq!(result, vec![1, 2, 0]);
    }


    // Property-based test: for any randomly generated row of pixel values (between 0 and 255),
    // the function encodes the row with GRP RLE compression, then decodes it back again.
    // The output must exactly match the original input.
    // This ensures our encoder and decoder are inverses of each other and that the RLE logic
    // works across a wide variety of input patterns, including edge cases we might not think to test manually.
    //
    // proptest generates hundreds of random rows (length 0 to 127) and runs the test for each.
    proptest! {
        #[test]
        fn prop_encode_decode_roundtrip(row in proptest::collection::vec(0u8..=255, 0..128)) {
            let width = row.len();
            let encoded = encode_grp_rle_row(&row, &CompressionType::Blizzard);
            let decoded = decode_grp_rle_row(&encoded, width);
            prop_assert_eq!(decoded, row);
        }
    }
}
