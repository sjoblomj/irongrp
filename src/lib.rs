use clap::{Parser, ValueEnum, ValueHint};
use clap_complete::Shell;
use simplelog::LevelFilter;
use std::cmp::Ordering;
use std::fmt;
use std::fs;

pub mod analyse;
pub mod error;
pub mod grp;
pub mod png;

pub use error::{Error, Result};
use error::InFile;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Path to the GRP file, or directory containing PNG files
    #[arg(long, short='i', value_hint = ValueHint::AnyPath,
          required_unless_present = "generator")]
    pub input_path: Option<String>,

    /// Path to the palette file.
    #[arg(long, short='p', value_hint = ValueHint::FilePath)]
    pub pal_path: Option<String>,

    /// Output directory if input is a GRP file,
    /// or output file if input is a directory
    #[arg(long, short='o', value_hint = ValueHint::AnyPath,
          required_if_eq_any = [("mode", "grp-to-png"), ("mode", "png-to-grp")])]
    pub output_path: Option<String>,

    /// Mode of operation.
    #[arg(long, short='m', value_enum, required_unless_present = "generator")]
    pub mode: Option<OperationMode>,

    /// Compression type to use when creating GRP files.
    /// If omitted or set to 'auto', it will use 'normal'
    /// compression, unless any of the input PNG file names
    /// contains the string "uncompressed" or "war1".
    /// If so, it will use the corresponding compression.
    #[arg(long, value_enum, default_value_t = CompressionType::Auto)]
    pub compression_type: CompressionType,

    /// Output all frames in one image. GRPs cannot be
    /// created back from tiled images.
    #[arg(long)]
    pub tiled: bool,

    /// Only applicable when using the 'tiled' argument.
    /// Maximum width in pixels of the output tiled image.
    /// If this is less than the maximum frame width of
    /// the GRP itself, this value will be ignored.
    #[arg(long, requires = "tiled")]
    pub max_width: Option<u32>,

    /// Only outputs or analyses the given frame number.
    #[arg(long, conflicts_with = "tiled")]
    pub frame_number: Option<u16>,

    /// Output the data of the given row number for the given frame.
    #[arg(long, requires = "frame_number")]
    pub analyse_row_number: Option<u8>,

    /// Enable transparency in the PNG images. The default
    /// behaviour is to use index 0 in the palette.
    #[arg(long)]
    pub use_transparency: bool,

    /// Logging level
    #[arg(long, value_enum, default_value_t = LogLevel::Info)]
    pub log_level: LogLevel,

    #[arg(long = "generate-shell-completions", value_enum, help = "Generate shell completions")]
    pub generator: Option<Shell>,
}

#[derive(Clone, ValueEnum, PartialEq)]
pub enum OperationMode {
    GrpToPng,
    PngToGrp,
    AnalyseGrp,
}

#[derive(Clone, ValueEnum, PartialEq, Debug)]
pub enum CompressionType {
    Normal,
    Optimised,
    Uncompressed,
    War1,
    Auto,
}

#[derive(Clone, ValueEnum, Debug)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}
impl fmt::Display for CompressionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl From<LogLevel> for LevelFilter {
    fn from(level: LogLevel) -> LevelFilter {
        match level {
            LogLevel::Error => LevelFilter::Error,
            LogLevel::Warn  => LevelFilter::Warn,
            LogLevel::Info  => LevelFilter::Info,
            LogLevel::Debug => LevelFilter::Debug,
            LogLevel::Trace => LevelFilter::Trace,
        }
    }
}

/// Returns all PNG files in the given directory.
pub fn list_png_files(dir: &str) -> Result<Vec<String>> {
    let mut entries: Vec<_> = fs::read_dir(dir).in_file(dir)?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()?.to_str()?.eq_ignore_ascii_case("png") {
                path.to_str().map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();

    if entries.is_empty() {
        return Err(Error::InvalidArgument(format!("No PNG files found in directory '{}'", dir)));
    }
    if entries.len() > u16::MAX as usize {
        return Err(Error::InvalidArgument(format!(
            "Too many PNGs found in directory '{}'! Found {} PNGs, but cannot handle more than {}",
            dir, entries.len(), u16::MAX)))
    }
    entries.sort_by(|a, b| natural_cmp(a, b));
    Ok(entries)
}

/// Compares strings so that runs of digits are ordered by their numeric value, so that e.g.
/// "frame_999.png" comes before "frame_1000.png". Strings that only differ in leading zeros,
/// such as "frame_01" and "frame_1", fall back to ordinary string comparison.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a_rest, mut b_rest) = (a, b);
    loop {
        let (Some(a_char), Some(b_char)) = (a_rest.chars().next(), b_rest.chars().next()) else {
            return a_rest.len().cmp(&b_rest.len()).then_with(|| a.cmp(b));
        };
        let ordering = if a_char.is_ascii_digit() && b_char.is_ascii_digit() {
            let (a_digits, a_tail) = split_leading_digits(a_rest);
            let (b_digits, b_tail) = split_leading_digits(b_rest);
            a_rest = a_tail;
            b_rest = b_tail;
            let (a_num, b_num) = (a_digits.trim_start_matches('0'), b_digits.trim_start_matches('0'));
            a_num.len().cmp(&b_num.len()).then_with(|| a_num.cmp(b_num))
        } else {
            a_rest = &a_rest[a_char.len_utf8()..];
            b_rest = &b_rest[b_char.len_utf8()..];
            a_char.cmp(&b_char)
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
}

fn split_leading_digits(s: &str) -> (&str, &str) {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s.split_at(end)
}

const UNCOMPRESSED_FILENAME: &str = "uncompressed";
const WAR1_FILENAME: &str = "war1";


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_cmp_orders_numbers_by_value() {
        assert_eq!(natural_cmp("frame_999.png",  "frame_1000.png"), Ordering::Less);
        assert_eq!(natural_cmp("frame_1000.png", "frame_999.png"),  Ordering::Greater);
        assert_eq!(natural_cmp("frame_002.png",  "frame_010.png"),  Ordering::Less);
        assert_eq!(natural_cmp("2.png",          "10.png"),         Ordering::Less);
        assert_eq!(natural_cmp("frame_10_b",     "frame_10_a"),     Ordering::Greater);
    }

    #[test]
    fn natural_cmp_compares_non_digits_as_usual() {
        assert_eq!(natural_cmp("a",       "b"),       Ordering::Less);
        assert_eq!(natural_cmp("frame",   "frame_1"), Ordering::Less);
        assert_eq!(natural_cmp("frame_1", "frame"),   Ordering::Greater);
        assert_eq!(natural_cmp("same_1",  "same_1"),  Ordering::Equal);
        assert_eq!(natural_cmp("åäö_2",   "åäö_10"),  Ordering::Less);
    }

    #[test]
    fn natural_cmp_is_a_total_order_for_leading_zeros() {
        // Numerically equal, so they fall back to string comparison rather than being Equal
        assert_eq!(natural_cmp("frame_01", "frame_1"),  Ordering::Less);
        assert_eq!(natural_cmp("frame_1",  "frame_01"), Ordering::Greater);
        // Leading zeros only decide when the strings are otherwise equal
        assert_eq!(natural_cmp("frame_1a", "frame_01b"), Ordering::Less);
    }

    #[test]
    fn natural_cmp_handles_numbers_too_large_for_integers() {
        assert_eq!(natural_cmp("99999999999999999999999", "100000000000000000000000"), Ordering::Less);
    }

    #[test]
    fn list_png_files_orders_frames_numerically() {
        let temp_dir = tempfile::tempdir().unwrap();
        // The same naming scheme as when writing PNGs, with more frames than 3 digits can hold
        for i in (0..=1001).rev() {
            std::fs::write(temp_dir.path().join(format!("frame_{:03}.png", i)), []).unwrap();
        }
        std::fs::write(temp_dir.path().join("not_a_png.txt"), []).unwrap();

        let files = list_png_files(temp_dir.path().to_str().unwrap()).unwrap();

        let names: Vec<String> = files.iter()
            .map(|f| std::path::Path::new(f).file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        let expected: Vec<String> = (0..=1001).map(|i| format!("frame_{:03}.png", i)).collect();
        assert_eq!(names, expected);
    }
}
