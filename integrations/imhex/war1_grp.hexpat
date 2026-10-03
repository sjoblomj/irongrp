import std.io;

struct GrpHeader {
    u16 frame_count;
    u8  max_width;
    u8  max_height;
};
GrpHeader grpHeader @ 0x00;

struct GrpFrameHeader {
    u8 x_offset [[color("FFF")]];
    u8 y_offset [[color("000")]];
    u8 width    [[color("FF00FF")]];
    u8 height   [[color("00FFFF")]];
    u32 image_data_offset [[color("FAFAFA")]];
    u8  image_data[height * width] @ image_data_offset;
};

GrpFrameHeader grpFrameHeader[grpHeader.frame_count] @ 0x04;

std::print("Frames: {}", grpHeader.frame_count);
