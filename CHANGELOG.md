# Changelog

All notable changes to this project will be documented in this file.

## [0.5] - 2025-06-19

### Added
- ImHex pattern language definitions for Normal GRPs, Uncompressed GRPs and WarCraft I style GRPs.
- Added yazi integration.
- Included fallback greyscale palette.
- Added a Readme file.
- Added a logo.

### Changed
- Moved the PNG handling to an external library.
- Better logging: Introduced logging library and the log level 'trace'.

### Removed
- Removed some Optimisation schemes from CompressionType Optimised. This makes it slightly less optimised but identical to how Blizzard did it, and the code is less complex.



## [0.4] - 2025-05-10

### Added
- Support for Extended Uncompressed GRPs. This allows for Uncompressed GRPs to have frames with a width up to 512 pixels.
- Support for WarCraft I Uncompressed GRPs.
- Shell completion.
- More tests.
- Boundary checks for width, height, offsets and frame count.

### Changed
- When extracting PNGs from an Uncompressed GRP or WarCraft I style GRP, IronGRP will now name them "uncompressed_frame_xxx.png" or "war1_frame_xxx.png", respectively. When converting PNGs to GRP, if no CompressionType is given (or if CompressionType Auto is given), IronGRP will create an Uncompressed GRP if any of the input filenames contains "uncompressed", and create a WarCraft I style GRP if any of the input filenames contains "war1".
- Renamed the values of the CompressionType to make more sense.
- Some refactoring to make code more reusable.



## [0.3] - 2025-04-19

### Added
- Support for converting to and from Uncompressed GRPs.
- Will now print which frames are identical when extracting frames from a GRP.
- Caching of palette lookups. This gives a speedup of over 80% on larger GRPs.

### Changed
- If requesting a tiled image with a max-width that is too low to fit one frame, the resulting image will now be 1 column wide and as big as the frame. Previously, it would in this case behave as if no max-width was given.



## [0.2] - 2025-04-15

### Added
- Now detects duplicated GRP frames and reuses them to save space.
- Will now reuse data row overlaps when using CompressionType Optimised.
- Added `--frame-number` and `--analyse-row-number` options. The former allows for only outputting the given frame, or to do more thorough analysis of the given frame. The latter is for analysing a specific row in the given frame.

### Changed
- Fixed a bug where the decoding would sometimes be fed too little data and thus decode incorrectly.
- Fixed an integer overflow bug.
- Fixed bug where reused frames would erroneously reuse offsets.
- Made the encoding algorithm closer to Blizzard's original algorithm in some cases.
- More efficient IO handling of GRP files.
- Now handles PNGs with alpha channels, in the sense that fully transparent pixels will be set to use palette index 0, and any non-opaque pixels will have its alpha value ignored.
- Improved and more consistent logging. Also prints how much time an operation took.



## [0.1] - 2025-04-03

### Added
- First version of program. Can convert from GRP to PNGs, and from PNGs to GRP. Can create tiled PNGs. Can analyse GRPs.
