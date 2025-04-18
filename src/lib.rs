use std::fmt;
use std::fs;
use std::sync::OnceLock;
use clap::{Parser, ValueEnum};

pub mod analyse;
pub mod grp;
pub mod png;

pub static LOG_LEVEL: OnceLock<LogLevel> = OnceLock::new();

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Path to the GRP file, or directory containing PNG files
    #[arg(long, short='i')]
    pub input_path: String,

    /// Path to the palette file.
    #[arg(long, short='p')]
    pub pal_path: Option<String>,

    /// Output directory if input is a GRP file,
    /// or output file if input is a directory
    #[arg(long, short='o')]
    pub output_path: Option<String>,

    /// Mode of operation.
    #[arg(long, short='m', required=true, value_enum, default_value_t = OperationMode::GrpToPng)]
    pub mode: OperationMode,

    /// Compression type to use when creating GRP files.
    #[arg(long, value_enum, default_value_t = CompressionType::Blizzard)]
    pub compression_type: CompressionType,

    /// Output all frames in one image. GRPs cannot be
    /// created back from tiled images.
    #[arg(long)]
    pub tiled: bool,

    /// Only applicable when using the 'tiled' argument.
    /// Maximum width in pixels of the output tiled image.
    /// If this is less than the maximum frame width of
    /// the GRP itself, this value will be ignored.
    #[arg(long)]
    pub max_width: Option<u32>,

    /// Only outputs or analyses the given frame number.
    #[arg(long)]
    pub frame_number: Option<u16>,

    /// Output the data of the given row number for the given frame.
    #[arg(long)]
    pub analyse_row_number: Option<u8>,

    /// Enable transparency in the PNG images. Default
    /// behavior is to use index 0 in the palette.
    #[arg(long)]
    pub use_transparency: bool,

    /// Logging level
    #[arg(long, value_enum, default_value_t = LogLevel::Info)]
    pub log_level: LogLevel,
}

#[derive(Clone, ValueEnum, PartialEq)]
pub enum OperationMode {
    GrpToPng,
    PngToGrp,
    AnalyseGrp,
}

#[derive(Clone, ValueEnum, PartialEq, Debug)]
pub enum CompressionType {
    Blizzard,
    Optimised,
    None,
}

#[derive(Clone, ValueEnum, Debug)]
pub enum LogLevel {
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


pub fn log(level: LogLevel, message: &str) {
    let level_order = |lvl: &LogLevel| match lvl {
        LogLevel::Debug => 0,
        LogLevel::Info  => 1,
        LogLevel::Warn  => 2,
        LogLevel::Error => 3,
    };

    if let Some(current_level) = LOG_LEVEL.get() {
        if level_order(&level) >= level_order(current_level) {
            println!("[{level}] {message}");
        }
    }
}


/// Returns all PNG files in the given directory.
pub fn list_png_files(dir: &str) -> std::io::Result<Vec<String>> {
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
