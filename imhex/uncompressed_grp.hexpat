import std.io;

struct GrpHeader {
    u16 frame_count;
    u16 max_width;
    u16 max_height;
};
GrpHeader grpHeader @ 0x00;

struct GrpFrameHeader {
    u8 x_offset [[color("FFF")]];
    u8 y_offset [[color("000")]];
    u8 width  [[color("FF00FF")]];
    u8 height [[color("00FFFF")]];
    u32 image_data_offset [[color("FAFAFA")]];

    if ((image_data_offset &  0x80000000) != 0) {
        // Large Uncompressed GRP. The high bit in the offset
        // signals that the width is greater than 256 pixels
        u32 offset = image_data_offset & 0x7FFFFFFF;
        u8 image_data[height * (width + 256)] @ offset;

    } else {
        // Normal Uncompressed GRP.
        u8 image_data[height * width] @ image_data_offset;
    }
};

GrpFrameHeader grpFrameHeader[grpHeader.frame_count] @ 0x06;

std::print("Frames {} ", grpHeader.frame_count);
