//! LSM files saved by Zeiss ZEN black / AIM (LSM 510 / 710 / 780 / 880 /
//! 900 / 980 confocals).
//!
//! An LSM file is a TIFF with a private tag, 34412 (CZ_LSMINFO), that holds
//! the dimensions, voxel sizes and file offsets of further structures: the
//! channel colours and names, the time stamps, the position lists and the
//! scan information (a tagged tree with the objective, lasers, and for every
//! track its detectors, pinholes, filters and illumination). Images come in
//! pairs of IFDs: the full-resolution image, then a thumbnail
//! (NewSubfileType = 1), which we skip. All channels of a Z/T position share
//! one IFD, as separate samples; the common layout is planar (one set of
//! strips per channel), chunky is handled too.
//!
//! The plugin only indexes the file; each page is handed to the host as a
//! recipe of strips, which decodes them (raw, LZW with or without the
//! predictor, zlib, PackBits) natively.
//!
//! Quirks handled, after Christoph Gohlke's tifffile (BSD-3-Clause), which
//! was the reference for the structure layouts and the quirks:
//! - SamplesPerPixel is often 3 for a two-channel image; the channel count
//!   comes from CZ_LSMINFO and the surplus strips are empty.
//! - StripByteCounts of compressed files hold the uncompressed sizes; the
//!   stored sizes are derived from the distance to the next strip.
//! - Files over 4 GiB wrap their 32-bit StripOffsets; the high bits are
//!   restored in physical order.
//! - A predictor tag on uncompressed data (a LSM 510 bug) is ignored.

use std::collections::BTreeSet;

use tifmrcview_plugin_sdk::tiff::{self, tag};
use tifmrcview_plugin_sdk::{
    bail, file, flags, Channel, Chunk, Codec, Context, DType, Format, Group, Info, Recipe, Result, Series,
};

/// The private CZ_LSMINFO tag.
const CZ_LSMINFO: u16 = 34412;
/// `MagicNumber` of the two structure versions.
const MAGICS: [u32; 2] = [50350412, 67127628];
/// Scan information bigger than this is cut off (a real one has some tens of KiB).
const MAX_SCAN_INFO: usize = 8 << 20;

const DEFAULT_COLORS: [[u8; 3]; 7] =
    [[0, 255, 0], [255, 0, 255], [0, 255, 255], [255, 0, 0], [0, 0, 255], [255, 255, 0], [255, 255, 255]];

// ---------------------------------------------------------------- CZ_LSMINFO

#[derive(Clone, Copy)]
enum Kind {
    U16,
    U32,
    I32,
    F64,
}
use Kind::*;

/// Name, offset and type of the fields of CZ_LSMINFO that are not just
/// reserved space, in file order.
const LSMINFO_FIELDS: &[(&str, usize, Kind)] = &[
    ("MagicNumber", 0, U32),
    ("StructureSize", 4, I32),
    ("DimensionX", 8, I32),
    ("DimensionY", 12, I32),
    ("DimensionZ", 16, I32),
    ("DimensionChannels", 20, I32),
    ("DimensionTime", 24, I32),
    ("DataType", 28, I32),
    ("ThumbnailX", 32, I32),
    ("ThumbnailY", 36, I32),
    ("VoxelSizeX", 40, F64),
    ("VoxelSizeY", 48, F64),
    ("VoxelSizeZ", 56, F64),
    ("OriginX", 64, F64),
    ("OriginY", 72, F64),
    ("OriginZ", 80, F64),
    ("ScanType", 88, U16),
    ("SpectralScan", 90, U16),
    ("TypeOfData", 92, U32),
    ("OffsetVectorOverlay", 96, U32),
    ("OffsetInputLut", 100, U32),
    ("OffsetOutputLut", 104, U32),
    ("OffsetChannelColors", 108, U32),
    ("TimeIntervall", 112, F64),
    ("OffsetChannelDataTypes", 120, U32),
    ("OffsetScanInformation", 124, U32),
    ("OffsetKsData", 128, U32),
    ("OffsetTimeStamps", 132, U32),
    ("OffsetEventList", 136, U32),
    ("OffsetRoi", 140, U32),
    ("OffsetBleachRoi", 144, U32),
    ("OffsetNextRecording", 148, U32),
    ("DisplayAspectX", 152, F64),
    ("DisplayAspectY", 160, F64),
    ("DisplayAspectZ", 168, F64),
    ("DisplayAspectTime", 176, F64),
    ("OffsetMeanOfRoisOverlay", 184, U32),
    ("OffsetTopoIsolineOverlay", 188, U32),
    ("OffsetTopoProfileOverlay", 192, U32),
    ("OffsetLinescanOverlay", 196, U32),
    ("ToolbarFlags", 200, U32),
    ("OffsetChannelWavelength", 204, U32),
    ("OffsetChannelFactors", 208, U32),
    ("ObjectiveSphereCorrection", 212, F64),
    ("OffsetUnmixParameters", 220, U32),
    ("OffsetAcquisitionParameters", 224, U32),
    ("OffsetCharacteristics", 228, U32),
    ("OffsetPalette", 232, U32),
    ("TimeDifferenceX", 236, F64),
    ("TimeDifferenceY", 244, F64),
    ("TimeDifferenceZ", 252, F64),
    ("InternalUse1", 260, U32),
    ("DimensionP", 264, I32),
    ("DimensionM", 268, I32),
    ("OffsetTilePositions", 336, U32),
    ("OffsetPositions", 376, U32),
];

/// The CZ_LSMINFO structure. Fields past `StructureSize` read as 0.
struct LsmInfo(Vec<u8>);

impl LsmInfo {
    fn parse(mut raw: Vec<u8>) -> Result<LsmInfo> {
        if raw.len() < 8 {
            bail!("LSM 文件损坏：CZ_LSMINFO 太短");
        }
        let magic = u32::from_le_bytes(raw[..4].try_into().unwrap());
        if !MAGICS.contains(&magic) {
            bail!("LSM 文件损坏：CZ_LSMINFO 标识不对");
        }
        let size = u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize;
        raw.truncate(size.clamp(8, raw.len()));
        Ok(LsmInfo(raw))
    }

    fn bytes<const N: usize>(&self, o: usize) -> Option<[u8; N]> {
        self.0.get(o..o + N)?.try_into().ok()
    }
    fn u16_(&self, o: usize) -> u16 {
        self.bytes(o).map_or(0, u16::from_le_bytes)
    }
    fn u32_(&self, o: usize) -> u32 {
        self.bytes(o).map_or(0, u32::from_le_bytes)
    }
    fn i32_(&self, o: usize) -> i32 {
        self.bytes(o).map_or(0, i32::from_le_bytes)
    }
    fn f64_(&self, o: usize) -> f64 {
        self.bytes(o).map_or(0.0, f64::from_le_bytes)
    }

    fn magic(&self) -> u32 {
        self.u32_(0)
    }
    fn dim_z(&self) -> i32 {
        self.i32_(16)
    }
    fn dim_c(&self) -> i32 {
        self.i32_(20)
    }
    fn dim_t(&self) -> i32 {
        self.i32_(24)
    }
    fn data_type(&self) -> i32 {
        self.i32_(28)
    }
    /// Voxel size in metres.
    fn voxel(&self) -> [f64; 3] {
        [self.f64_(40), self.f64_(48), self.f64_(56)]
    }
    fn scan_type(&self) -> u16 {
        self.u16_(88)
    }
    fn spectral(&self) -> bool {
        self.u16_(90) != 0
    }
    fn time_interval(&self) -> f64 {
        self.f64_(112)
    }
    fn off_channel_colors(&self) -> u64 {
        self.u32_(108) as u64
    }
    fn off_scan_info(&self) -> u64 {
        self.u32_(124) as u64
    }
    fn off_time_stamps(&self) -> u64 {
        self.u32_(132) as u64
    }
    fn off_channel_wavelength(&self) -> u64 {
        self.u32_(204) as u64
    }
    fn dim_p(&self) -> i32 {
        self.i32_(264)
    }
    fn dim_m(&self) -> i32 {
        self.i32_(268)
    }
    fn off_tile_positions(&self) -> u64 {
        self.u32_(336) as u64
    }
    fn off_positions(&self) -> u64 {
        self.u32_(376) as u64
    }

    /// Every field as (name, value) for the "all settings" group.
    fn listing(&self) -> Vec<(&'static str, String)> {
        LSMINFO_FIELDS
            .iter()
            .filter(|(name, o, _)| !name.starts_with("Offset") && *o < self.0.len())
            .map(|&(name, o, kind)| {
                let v = match kind {
                    U16 => self.u16_(o).to_string(),
                    U32 => self.u32_(o).to_string(),
                    I32 => self.i32_(o).to_string(),
                    F64 => num_f(self.f64_(o)),
                };
                (name, v)
            })
            .collect()
    }
}

/// Which extra structures follow the pages in the scan type's dimension order.
/// Scan types 0, 3, 6, 7 keep Z in the stack; 3, 4, 6, 7, 9 have a time axis;
/// in the others the extra dimension lives in the height of the page.
fn z_in_stack(scan_type: u16) -> bool {
    matches!(scan_type, 0 | 3 | 6 | 7) || scan_type > 10
}
fn t_in_stack(scan_type: u16) -> bool {
    matches!(scan_type, 3 | 4 | 6 | 7 | 9) || scan_type > 10
}

// ---------------------------------------------------------------- scan information

/// A value of the scan information tree.
#[derive(Clone, Debug)]
enum Val {
    S(String),
    I(i32),
    F(f64),
}

impl Val {
    fn to_text(&self) -> String {
        match self {
            Val::S(s) => s.clone(),
            Val::I(i) => i.to_string(),
            Val::F(f) => num_f(*f),
        }
    }
}

/// A block of the scan information: the recording, or an array (`Tracks`…)
/// or one of its elements (`Track`…). Arrays hold their elements as `kids`.
#[derive(Debug)]
struct Node {
    code: u32,
    attrs: Vec<(u32, Val)>,
    kids: Vec<Node>,
}

// Recording.
const REC: u32 = 0x1000_0000;
const REC_NAME: u32 = 0x1000_0001;
const REC_DESCRIPTION: u32 = 0x1000_0002;
const REC_NOTES: u32 = 0x1000_0003;
const REC_OBJECTIVE: u32 = 0x1000_0004;
const REC_SCAN_MODE: u32 = 0x1000_0008;
const REC_ZOOM_X: u32 = 0x1000_0016;
const REC_PLANE_WIDTH: u32 = 0x1000_001F;
const REC_PLANE_HEIGHT: u32 = 0x1000_0020;
const REC_SAMPLE0_TIME: u32 = 0x1000_0036;
const REC_USER: u32 = 0x1000_0047;
// Arrays and their elements.
const LASERS: u32 = 0x3000_0000;
const LASER: u32 = 0x5000_0000;
const LASER_NAME: u32 = 0x5000_0001;
const LASER_ACQUIRE: u32 = 0x5000_0002;
const LASER_POWER: u32 = 0x5000_0003;
const TRACKS: u32 = 0x2000_0000;
const TRACK: u32 = 0x4000_0000;
const TRACK_MULTIPLEX_TYPE: u32 = 0x4000_0001;
const TRACK_SAMPLING_MODE: u32 = 0x4000_0003;
const TRACK_SAMPLING_METHOD: u32 = 0x4000_0004;
const TRACK_SAMPLING_NUMBER: u32 = 0x4000_0005;
const TRACK_ACQUIRE: u32 = 0x4000_0006;
const TRACK_NAME: u32 = 0x4000_000C;
const TRACK_COLLIMATOR1_NAME: u32 = 0x4000_000D;
const TRACK_COLLIMATOR1_POS: u32 = 0x4000_000E;
const TRACK_COLLIMATOR2_NAME: u32 = 0x4000_000F;
const TRACK_COLLIMATOR2_POS: u32 = 0x4000_0010;
const TRACK_PIXEL_TIME: u32 = 0x4000_0019;
const DET_CHANNELS: u32 = 0x6000_0000;
const DET: u32 = 0x7000_0000;
const DET_GAIN_FIRST: u32 = 0x7000_0003;
const DET_GAIN_LAST: u32 = 0x7000_0004;
const DET_AMP_GAIN_FIRST: u32 = 0x7000_0005;
const DET_AMP_OFFS_FIRST: u32 = 0x7000_0007;
const DET_PINHOLE: u32 = 0x7000_0009;
const DET_ACQUIRE: u32 = 0x7000_000B;
const DET_POINT_DETECTOR: u32 = 0x7000_000C;
const DET_PINHOLE_NAME: u32 = 0x7000_000E;
const DET_FILTER_SET: u32 = 0x7000_000F;
const DET_FILTER: u32 = 0x7000_0010;
const DET_CHANNEL_NAME: u32 = 0x7000_0014;
const DET_SPECTRAL_CHANNELS: u32 = 0x7000_0021;
const DET_SPI_START: u32 = 0x7000_0022;
const DET_SPI_STOP: u32 = 0x7000_0023;
const DET_DYE_NAME: u32 = 0x7000_0026;
const ILLUM_CHANNELS: u32 = 0x8000_0000;
const ILLUM_NAME: u32 = 0x9000_0001;
const ILLUM_POWER: u32 = 0x9000_0002;
const ILLUM_WAVELENGTH: u32 = 0x9000_0003;
/// Sic: the specification spells it "Aquire".
const ILLUM_ACQUIRE: u32 = 0x9000_0004;
const BEAM_SPLITTERS: u32 = 0xA000_0000;
const BS_FILTER_SET: u32 = 0xB000_0001;
const BS_FILTER: u32 = 0xB000_0002;
const DATA_CHANNELS: u32 = 0xC000_0000;
const DATA_BITS: u32 = 0xD000_0006;

impl Node {
    fn attr(&self, code: u32) -> Option<&Val> {
        self.attrs.iter().find(|(c, _)| *c == code).map(|(_, v)| v)
    }
    /// A non-empty string attribute (ZEN pads empty strings with a space).
    fn text(&self, code: u32) -> Option<String> {
        match self.attr(code)? {
            Val::S(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            _ => None,
        }
    }
    fn float(&self, code: u32) -> Option<f64> {
        match self.attr(code)? {
            Val::I(i) => Some(*i as f64),
            Val::F(f) => Some(*f),
            Val::S(_) => None,
        }
    }
    fn int(&self, code: u32) -> Option<i64> {
        self.float(code).map(|f| f as i64)
    }
    /// The elements of the array `code` (empty when there is none).
    fn list(&self, code: u32) -> &[Node] {
        self.kids.iter().find(|k| k.code == code).map_or(&[], |k| &k.kids)
    }
}

fn is_array(code: u32) -> bool {
    matches!(code, 0x2000_0000 | 0x3000_0000 | 0x6000_0000 | 0x8000_0000 | 0xA000_0000 | 0xC000_0000 | 0x1100_0000 | 0x1300_0000)
}

fn is_struct(code: u32) -> bool {
    matches!(code, 0x4000_0000 | 0x5000_0000 | 0x7000_0000 | 0x9000_0000 | 0xB000_0000 | 0xD000_0000 | 0x1200_0000 | 0x1400_0000)
}

/// Parses the scan information: a sequence of (entry, type, size, payload)
/// records where array and element entries open a block and 0xFFFFFFFF closes
/// the innermost one. Types: 2 string, 4 long, 5 double.
fn parse_scan_info(buf: &[u8]) -> Option<Node> {
    let u32_at = |o: usize| buf.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if u32_at(0)? != REC {
        return None;
    }
    let mut stack = vec![Node { code: REC, attrs: vec![], kids: vec![] }];
    let mut pos = 12;
    while let (Some(entry), Some(typ), Some(size)) = (u32_at(pos), u32_at(pos + 4), u32_at(pos + 8)) {
        pos += 12;
        let payload = buf.get(pos..pos.saturating_add(size as usize));
        let Some(payload) = payload else { break };
        pos += size as usize;
        if entry == 0xFFFF_FFFF {
            let done = stack.pop()?;
            match stack.last_mut() {
                Some(parent) => parent.kids.push(done),
                None => return Some(done),
            }
        } else if is_array(entry) || is_struct(entry) {
            stack.push(Node { code: entry, attrs: vec![], kids: vec![] });
        } else {
            let v = match typ {
                2 => Val::S(latin_or_utf8(payload)),
                4 if payload.len() >= 4 => Val::I(i32::from_le_bytes(payload[..4].try_into().unwrap())),
                5 if payload.len() >= 8 => Val::F(f64::from_le_bytes(payload[..8].try_into().unwrap())),
                _ => Val::I(0),
            };
            stack.last_mut()?.attrs.push((entry, v));
        }
    }
    // A truncated tree: close what is open.
    while stack.len() > 1 {
        let done = stack.pop()?;
        stack.last_mut()?.kids.push(done);
    }
    stack.pop()
}

/// Names of the scan information entries (after tifffile's tables).
static ATTR_NAMES: &[(u32, &str)] = &[
    (0x10000001, "Name"),
    (0x10000002, "Description"),
    (0x10000003, "Notes"),
    (0x10000004, "Objective"),
    (0x10000005, "ProcessingSummary"),
    (0x10000006, "SpecialScanMode"),
    (0x10000007, "ScanType"),
    (0x10000008, "ScanMode"),
    (0x10000009, "NumberOfStacks"),
    (0x1000000A, "LinesPerPlane"),
    (0x1000000B, "SamplesPerLine"),
    (0x1000000C, "PlanesPerVolume"),
    (0x1000000D, "ImagesWidth"),
    (0x1000000E, "ImagesHeight"),
    (0x1000000F, "ImagesNumberPlanes"),
    (0x10000010, "ImagesNumberStacks"),
    (0x10000011, "ImagesNumberChannels"),
    (0x10000012, "LinscanXySize"),
    (0x10000013, "ScanDirection"),
    (0x10000014, "TimeSeries"),
    (0x10000015, "OriginalScanData"),
    (0x10000016, "ZoomX"),
    (0x10000017, "ZoomY"),
    (0x10000018, "ZoomZ"),
    (0x10000019, "Sample0X"),
    (0x1000001A, "Sample0Y"),
    (0x1000001B, "Sample0Z"),
    (0x1000001C, "SampleSpacing"),
    (0x1000001D, "LineSpacing"),
    (0x1000001E, "PlaneSpacing"),
    (0x1000001F, "PlaneWidth"),
    (0x10000020, "PlaneHeight"),
    (0x10000021, "VolumeDepth"),
    (0x10000023, "Nutation"),
    (0x10000034, "Rotation"),
    (0x10000035, "Precession"),
    (0x10000036, "Sample0time"),
    (0x10000037, "StartScanTriggerIn"),
    (0x10000038, "StartScanTriggerOut"),
    (0x10000039, "StartScanEvent"),
    (0x10000040, "StartScanTime"),
    (0x10000041, "StopScanTriggerIn"),
    (0x10000042, "StopScanTriggerOut"),
    (0x10000043, "StopScanEvent"),
    (0x10000044, "StopScanTime"),
    (0x10000045, "UseRois"),
    (0x10000046, "UseReducedMemoryRois"),
    (0x10000047, "User"),
    (0x10000048, "UseBcCorrection"),
    (0x10000049, "PositionBcCorrection1"),
    (0x10000050, "PositionBcCorrection2"),
    (0x10000051, "InterpolationY"),
    (0x10000052, "CameraBinning"),
    (0x10000053, "CameraSupersampling"),
    (0x10000054, "CameraFrameWidth"),
    (0x10000055, "CameraFrameHeight"),
    (0x10000056, "CameraOffsetX"),
    (0x10000057, "CameraOffsetY"),
    (0x10000059, "RtBinning"),
    (0x1000005A, "RtFrameWidth"),
    (0x1000005B, "RtFrameHeight"),
    (0x1000005C, "RtRegionWidth"),
    (0x1000005D, "RtRegionHeight"),
    (0x1000005E, "RtOffsetX"),
    (0x1000005F, "RtOffsetY"),
    (0x10000060, "RtZoom"),
    (0x10000061, "RtLinePeriod"),
    (0x10000062, "Prescan"),
    (0x10000063, "ScanDirectionZ"),
    (0x12000001, "Name"),
    (0x12000002, "Description"),
    (0x12000003, "Interval"),
    (0x12000004, "TriggerIn"),
    (0x12000005, "TriggerOut"),
    (0x12000006, "ActivationTime"),
    (0x12000007, "ActivationNumber"),
    (0x14000001, "Name"),
    (0x14000002, "Description"),
    (0x14000003, "TriggerIn"),
    (0x14000004, "TriggerOut"),
    (0x40000001, "MultiplexType"),
    (0x40000002, "MultiplexOrder"),
    (0x40000003, "SamplingMode"),
    (0x40000004, "SamplingMethod"),
    (0x40000005, "SamplingNumber"),
    (0x40000006, "Acquire"),
    (0x40000007, "SampleObservationTime"),
    (0x4000000B, "TimeBetweenStacks"),
    (0x4000000C, "Name"),
    (0x4000000D, "Collimator1Name"),
    (0x4000000E, "Collimator1Position"),
    (0x4000000F, "Collimator2Name"),
    (0x40000010, "Collimator2Position"),
    (0x40000011, "IsBleachTrack"),
    (0x40000012, "IsBleachAfterScanNumber"),
    (0x40000013, "BleachScanNumber"),
    (0x40000014, "TriggerIn"),
    (0x40000015, "TriggerOut"),
    (0x40000016, "IsRatioTrack"),
    (0x40000017, "BleachCount"),
    (0x40000018, "SpiCenterWavelength"),
    (0x40000019, "PixelTime"),
    (0x40000021, "CondensorFrontlens"),
    (0x40000023, "FieldStopValue"),
    (0x40000024, "IdCondensorAperture"),
    (0x40000025, "CondensorAperture"),
    (0x40000026, "IdCondensorRevolver"),
    (0x40000027, "CondensorFilter"),
    (0x40000028, "IdTransmissionFilter1"),
    (0x40000029, "IdTransmission1"),
    (0x40000030, "IdTransmissionFilter2"),
    (0x40000031, "IdTransmission2"),
    (0x40000032, "RepeatBleach"),
    (0x40000033, "EnableSpotBleachPos"),
    (0x40000034, "SpotBleachPosx"),
    (0x40000035, "SpotBleachPosy"),
    (0x40000036, "SpotBleachPosz"),
    (0x40000037, "IdTubelens"),
    (0x40000038, "IdTubelensPosition"),
    (0x40000039, "TransmittedLight"),
    (0x4000003A, "ReflectedLight"),
    (0x4000003B, "SimultanGrabAndBleach"),
    (0x4000003C, "BleachPixelTime"),
    (0x50000001, "Name"),
    (0x50000002, "Acquire"),
    (0x50000003, "Power"),
    (0x70000001, "IntegrationMode"),
    (0x70000002, "SpecialMode"),
    (0x70000003, "DetectorGainFirst"),
    (0x70000004, "DetectorGainLast"),
    (0x70000005, "AmplifierGainFirst"),
    (0x70000006, "AmplifierGainLast"),
    (0x70000007, "AmplifierOffsFirst"),
    (0x70000008, "AmplifierOffsLast"),
    (0x70000009, "PinholeDiameter"),
    (0x7000000A, "CountingTrigger"),
    (0x7000000B, "Acquire"),
    (0x7000000C, "PointDetectorName"),
    (0x7000000D, "AmplifierName"),
    (0x7000000E, "PinholeName"),
    (0x7000000F, "FilterSetName"),
    (0x70000010, "FilterName"),
    (0x70000013, "IntegratorName"),
    (0x70000014, "ChannelName"),
    (0x70000015, "DetectorGainBc1"),
    (0x70000016, "DetectorGainBc2"),
    (0x70000017, "AmplifierGainBc1"),
    (0x70000018, "AmplifierGainBc2"),
    (0x70000019, "AmplifierOffsetBc1"),
    (0x70000020, "AmplifierOffsetBc2"),
    (0x70000021, "SpectralScanChannels"),
    (0x70000022, "SpiWavelengthStart"),
    (0x70000023, "SpiWavelengthStop"),
    (0x70000026, "DyeName"),
    (0x70000027, "DyeFolder"),
    (0x90000001, "Name"),
    (0x90000002, "Power"),
    (0x90000003, "Wavelength"),
    (0x90000004, "Aquire"),
    (0x90000005, "DetchannelName"),
    (0x90000006, "PowerBc1"),
    (0x90000007, "PowerBc2"),
    (0xB0000001, "FilterSet"),
    (0xB0000002, "Filter"),
    (0xB0000003, "Name"),
    (0xD0000001, "Name"),
    (0xD0000003, "Acquire"),
    (0xD0000004, "Color"),
    (0xD0000005, "SampleType"),
    (0xD0000006, "BitsPerSample"),
    (0xD0000007, "RatioType"),
    (0xD0000008, "RatioTrack1"),
    (0xD0000009, "RatioTrack2"),
    (0xD000000A, "RatioChannel1"),
    (0xD000000B, "RatioChannel2"),
    (0xD000000C, "RatioConst1"),
    (0xD000000D, "RatioConst2"),
    (0xD000000E, "RatioConst3"),
    (0xD000000F, "RatioConst4"),
    (0xD0000010, "RatioConst5"),
    (0xD0000011, "RatioConst6"),
    (0xD0000012, "RatioFirstImages1"),
    (0xD0000013, "RatioFirstImages2"),
    (0xD0000014, "DyeName"),
    (0xD0000015, "DyeFolder"),
    (0xD0000016, "Spectrum"),
    (0xD0000017, "Acquire"),
];

fn attr_name(code: u32) -> String {
    match ATTR_NAMES.binary_search_by_key(&code, |&(c, _)| c) {
        Ok(i) => ATTR_NAMES[i].1.to_string(),
        Err(_) => format!("Entry0x{code:x}"),
    }
}

/// Name of an array or element block.
fn kind_name(code: u32) -> &'static str {
    match code {
        TRACKS => "Tracks",
        LASERS => "Lasers",
        DET_CHANNELS => "DetectionChannels",
        ILLUM_CHANNELS => "IlluminationChannels",
        BEAM_SPLITTERS => "BeamSplitters",
        DATA_CHANNELS => "DataChannels",
        0x1100_0000 => "Timers",
        0x1300_0000 => "Markers",
        TRACK => "Track",
        LASER => "Laser",
        DET => "DetectionChannel",
        0x9000_0000 => "IlluminationChannel",
        0xB000_0000 => "BeamSplitter",
        0xD000_0000 => "DataChannel",
        0x1200_0000 => "Timer",
        0x1400_0000 => "Marker",
        _ => "Block",
    }
}

/// Appends every attribute under `n`, with a path of block names.
fn flatten(g: &mut Group, path: &str, n: &Node) {
    for (code, v) in &n.attrs {
        let text = v.to_text();
        if !text.trim().is_empty() {
            g.push(&format!("{path}/{}", attr_name(*code)), text.trim());
        }
    }
    for k in &n.kids {
        if is_array(k.code) {
            for (i, e) in k.kids.iter().enumerate() {
                flatten(g, &format!("{path}/{}[{}]", kind_name(e.code), i + 1), e);
            }
        } else {
            flatten(g, &format!("{path}/{}", kind_name(k.code)), k);
        }
    }
}

// ---------------------------------------------------------------- small helpers

/// Windows-1252 / Latin-1 text unless it is valid UTF-8; stops at the first NUL.
fn latin_or_utf8(b: &[u8]) -> String {
    let b = &b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())];
    match std::str::from_utf8(b) {
        Ok(s) => s.trim().to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect::<String>().trim().to_string(),
    }
}

/// Trims float noise such as 0.030000000000000138 to a few decimals.
fn num_f(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let r = format!("{v:.4}");
    let r = if r.contains('.') { r.trim_end_matches('0').trim_end_matches('.') } else { r.as_str() };
    if (r == "0" || r == "-0") && v != 0.0 {
        format!("{v:.3e}")
    } else {
        r.to_string()
    }
}

/// "12.5%"-style helper: a number plus a unit.
fn with_unit(v: f64, unit: &str) -> String {
    format!("{} {unit}", num_f(v))
}

/// An OLE automation date (days since 1899-12-30) as "2005-05-27 11:09:32".
fn ole_date(d: f64) -> Option<String> {
    if !(1.0..2.9e6).contains(&d) {
        return None;
    }
    let days = d.floor() as i64 - 25569; // since 1970-01-01
    let secs = (((d - d.floor()) * 86400.0).round() as i64).min(86399);
    // Civil date from a day count (proleptic Gregorian).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + (month <= 2) as i64;
    Some(format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60))
}

fn scan_type_name(t: u16) -> &'static str {
    match t {
        0 => "Z 栈（XYZ）",
        1 => "Z 扫描（XZ 平面）",
        2 => "线扫描",
        3 => "时间序列（XY 平面）",
        4 => "时间序列（XZ 平面）",
        5 => "时间序列（ROI 平均）",
        6 => "时间序列 Z 栈（XYZ-T）",
        7 => "样条扫描",
        8 => "样条平面（XZ）",
        9 => "时间序列样条平面",
        10 => "点 / 时间序列点",
        _ => "未知",
    }
}

fn data_type_name(t: i32) -> &'static str {
    match t {
        0 => "各通道不同",
        1 => "8 位无符号整数",
        2 => "12 位无符号整数",
        5 => "32 位浮点",
        _ => "未知",
    }
}

fn sampling_mode_name(m: i64) -> &'static str {
    match m {
        0 => "单次采样",
        1 => "行平均",
        2 => "帧平均",
        _ => "未知",
    }
}

fn hex_color(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

// ---------------------------------------------------------------- other LSM structures

/// Channel colours and names (the "ChannelColors" block).
#[derive(Default)]
struct ChannelColors {
    colors: Vec<[u8; 3]>,
    names: Vec<String>,
}

fn read_channel_colors(off: u64) -> ChannelColors {
    let read = || -> Result<ChannelColors> {
        let head = file::read(off, 24)?;
        let u = |i: usize| u32::from_le_bytes(head[i * 4..i * 4 + 4].try_into().unwrap()) as u64;
        let (size, ncolors, nnames, coff, noff) = (u(0), u(1), u(2), u(3), u(4));
        if !(24..=1 << 20).contains(&size) || ncolors != nnames || ncolors > 4096 {
            bail!("通道颜色结构不对");
        }
        let block = file::read_at_most(off, size as usize)?;
        let mut out = ChannelColors::default();
        let colors = block.get(coff as usize..(coff + ncolors * 4) as usize).context("通道颜色被截断")?;
        out.colors = colors.chunks_exact(4).map(|c| [c[0], c[1], c[2]]).collect();
        // Names: (u32 length, bytes) in sequence.
        let mut p = noff as usize;
        while p + 4 <= block.len() && out.names.len() < ncolors as usize {
            let n = u32::from_le_bytes(block[p..p + 4].try_into().unwrap()) as usize;
            let Some(s) = block.get(p + 4..p + 4 + n) else { break };
            out.names.push(latin_or_utf8(s));
            p += 4 + n;
        }
        Ok(out)
    };
    if off < 8 {
        return ChannelColors::default();
    }
    read().unwrap_or_default()
}

/// Time stamps in seconds, one per image.
fn read_time_stamps(off: u64) -> Vec<f64> {
    let read = || -> Result<Vec<f64>> {
        let head = file::read(off, 8)?;
        let size = i32::from_le_bytes(head[..4].try_into().unwrap());
        let count = i32::from_le_bytes(head[4..8].try_into().unwrap());
        if count <= 0 || count > 1 << 24 || size as i64 != 8 + 8 * count as i64 {
            bail!("时间戳结构不对");
        }
        let b = file::read(off + 8, count as usize * 8)?;
        Ok(b.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect())
    };
    if off < 8 {
        return vec![];
    }
    read().unwrap_or_default()
}

/// A list of `n` records of `width` f64 after a u32 count (positions: 3, wavelengths: 2).
fn read_f64_records(off: u64, width: usize) -> Vec<Vec<f64>> {
    let read = || -> Result<Vec<Vec<f64>>> {
        let count = u32::from_le_bytes(file::read(off, 4)?[..4].try_into().unwrap()) as usize;
        if count == 0 || count > 1 << 20 {
            bail!("记录数不对");
        }
        let b = file::read(off + 4, count * width * 8)?;
        Ok(b.chunks_exact(width * 8)
            .map(|r| r.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect())
            .collect())
    };
    if off < 8 {
        return vec![];
    }
    read().unwrap_or_default()
}

// ---------------------------------------------------------------- the file

/// How the pixels of one page are stored (from the first image IFD).
struct Geom {
    w: u32,
    h: u32,
    rows_per_strip: u32,
    /// Strips per plane.
    strips: usize,
    /// Samples per pixel in the TIFF (may exceed the channel count).
    spp: u32,
    planar: u16,
    /// Channels that carry data.
    channels: u32,
    dtype: DType,
    codec: Codec,
    predictor: bool,
}

/// Where an IFD entry's values are in the file (they are read when needed).
#[derive(Clone, Copy)]
struct Loc {
    typ: u16,
    count: u64,
    at: u64,
}

/// What the plugin keeps of every IFD of the chain: not much, since LSM
/// files of time series have thousands of them.
struct IfdRef {
    reduced: bool,
    width: u64,
    offsets: Option<Loc>,
    counts: Option<Loc>,
}

/// Values larger than this are not loaded.
const MAX_VALUE: u64 = 64 << 20;
/// Guards against loops and absurd chains.
const MAX_IFDS: usize = 1 << 20;

impl Loc {
    /// The first `limit` integer values (BYTE … LONG8).
    fn read(&self, limit: usize) -> Result<Vec<u64>> {
        let size = tiff::type_size(self.typ);
        let n = (self.count as usize).min(limit);
        if size == 0 || size > 8 || self.count.saturating_mul(size) > MAX_VALUE {
            bail!("LSM 的 TIFF 标签值不对");
        }
        let b = file::read(self.at, n * size as usize)?;
        Ok(b.chunks_exact(size as usize)
            .map(|c| {
                let mut v = [0u8; 8];
                v[..c.len()].copy_from_slice(c);
                u64::from_le_bytes(v)
            })
            .collect())
    }
}

/// Walks the chain of IFDs (classic little-endian TIFF), noting only what the
/// plugin needs of each. A chain cut short by an interrupted acquisition ends
/// where it breaks.
fn walk_ifds(first: u64, size: u64) -> Result<Vec<IfdRef>> {
    let mut out = vec![];
    let mut seen = BTreeSet::new();
    let mut next = first;
    while next != 0 && next + 2 <= size && out.len() < MAX_IFDS && seen.insert(next) {
        let mut buf = file::read_at_most(next, 2 + 12 * 32 + 4)?;
        if buf.len() < 2 {
            break;
        }
        let n = u16::from_le_bytes([buf[0], buf[1]]) as usize;
        let need = 2 + n * 12 + 4;
        if n == 0 || next + need as u64 > size {
            if out.is_empty() {
                bail!("LSM 的第一个 TIFF 目录损坏");
            }
            break;
        }
        if buf.len() < need {
            buf = file::read(next, need)?;
        }
        let mut r = IfdRef { reduced: false, width: 0, offsets: None, counts: None };
        for k in 0..n {
            let e = &buf[2 + k * 12..14 + k * 12];
            let (tg, typ) = (u16::from_le_bytes([e[0], e[1]]), u16::from_le_bytes([e[2], e[3]]));
            let count = u32::from_le_bytes([e[4], e[5], e[6], e[7]]) as u64;
            let value = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
            let inline = count.saturating_mul(tiff::type_size(typ)) <= 4;
            let loc = Loc { typ, count, at: if inline { next + 2 + k as u64 * 12 + 8 } else { value as u64 } };
            match tg {
                tag::NEW_SUBFILE_TYPE => r.reduced = value & 1 != 0,
                tag::IMAGE_WIDTH => r.width = if typ == 3 { value as u16 as u64 } else { value as u64 },
                tag::STRIP_OFFSETS => r.offsets = Some(loc),
                tag::STRIP_BYTE_COUNTS => r.counts = Some(loc),
                _ => {}
            }
        }
        next = u32::from_le_bytes(buf[need - 4..need].try_into().unwrap()) as u64;
        out.push(r);
    }
    Ok(out)
}

pub struct Lsm {
    ifds: Vec<IfdRef>,
    /// Indices of the full-resolution IFDs, in file order.
    main: Vec<usize>,
    geom: Geom,
    /// Strip offsets of every IFD with the 4 GiB wrap undone (files over 4 GiB only).
    fixed: Option<Vec<Vec<u64>>>,
    /// First strip offset of every IFD, sorted (compressed files only).
    starts: Vec<u64>,
    /// First entry of `main` of this series, and how many pages it has.
    base: usize,
    pages: u32,
    info: Option<Info>,
}

impl Lsm {
    fn offsets(&self, ifd: usize) -> Result<Vec<u64>> {
        if let Some(f) = &self.fixed {
            return Ok(f[ifd].clone());
        }
        self.ifds[ifd].offsets.context("LSM 页面没有 StripOffsets")?.read(usize::MAX)
    }

    fn counts(&self, ifd: usize) -> Vec<u64> {
        self.ifds[ifd].counts.and_then(|l| l.read(usize::MAX).ok()).unwrap_or_default()
    }

    /// Offset of the first strip of an IFD.
    fn first_offset(&self, ifd: usize) -> Option<u64> {
        match &self.fixed {
            Some(f) => f[ifd].first().copied(),
            None => self.ifds[ifd].offsets?.read(1).ok()?.first().copied(),
        }
    }

    /// Stored length of strip `k` of a compressed page. The byte counts of LSM
    /// files hold the uncompressed sizes, so the length is the distance to
    /// whatever follows (the next strip, else the next image).
    fn stored_len(&self, offs: &[u64], counts: &[u64], k: usize) -> u64 {
        let off = offs[k];
        let uncompressed = counts.get(k).copied().unwrap_or(0);
        let end = match offs.get(k + 1).copied().filter(|&n| n > off) {
            Some(n) => n,
            None => {
                let i = self.starts.partition_point(|&s| s <= off);
                self.starts.get(i).copied().unwrap_or(off + uncompressed * 2)
            }
        };
        // LZW can expand a little, never to twice the size.
        end.min(file::size()).min(off + uncompressed * 2 + 1024).saturating_sub(off)
    }
}

/// The order in which the IFDs are physically stored, for undoing the 4 GiB
/// wrap: image / thumbnail pairs, with the time, position and tile loops
/// reordered from the order of the IFDs (tile outermost) to the order of
/// acquisition (time outermost).
fn physical_order(n: usize, paired: bool, thumb_first: bool, mpt: [usize; 3]) -> Vec<usize> {
    if !paired || n < 2 || n % 2 != 0 {
        return (0..n).collect();
    }
    let [m, p, t] = mpt;
    let pairs = n / 2;
    let outer = m * p * t;
    let at = |pair: usize, k: usize| pair * 2 + if thumb_first { 1 - k } else { k };
    if [m, p, t].iter().filter(|&&v| v > 1).count() < 2 || pairs % outer != 0 {
        return (0..pairs).flat_map(|i| [at(i, 0), at(i, 1)]).collect();
    }
    let inner = pairs / outer;
    let mut order = Vec::with_capacity(n);
    for it in 0..t {
        for ip in 0..p {
            for im in 0..m {
                for i in 0..inner {
                    let pair = ((im * p + ip) * t + it) * inner + i;
                    order.extend([at(pair, 0), at(pair, 1)]);
                }
            }
        }
    }
    order
}

/// Whether 32-bit offset `a` lies before `b` (by less than 2 GiB, modulo 2^32).
fn precedes(a: u64, b: u64) -> bool {
    (b as u32).wrapping_sub(a as u32).wrapping_sub(1) < (1 << 31) - 1
}

/// Adds 2^32 to the strip offsets each time they decrease (in physical order).
/// Empty strips (the surplus samples of a two-channel image) are left alone.
fn unwrap_offsets(offs: &mut [Vec<u64>], counts: &[Vec<u64>], order: &[usize]) {
    let (mut wrap, mut prev) = (0u64, 0u64);
    for &i in order {
        if offs[i].iter().all(|&o| o == 0) {
            // Incompletely written file.
            break;
        }
        for (k, o) in offs[i].iter_mut().enumerate() {
            if counts[i].get(k).copied().unwrap_or(1) == 0 {
                *o += wrap;
                continue;
            }
            let cur = *o;
            if cur < prev {
                wrap += 1 << 32;
            }
            *o = cur + wrap;
            prev = cur;
        }
    }
}

impl Lsm {
    fn build(series: u32) -> Result<Lsm> {
        let size = file::size();
        // The first IFD in full (with CZ_LSMINFO), then the rest of the chain.
        let head = tiff::parse(&mut tiff::file_reader, size, 1).map_err(|_| "不是有效的 LSM 文件")?;
        if !head.little || head.big {
            bail!("不是 Zeiss LSM 文件（需要经典的小端 TIFF）");
        }
        let tiff = &head;
        let ifd = &head.ifds[0];
        let raw = ifd.get(CZ_LSMINFO).context("不是 Zeiss LSM 文件（缺少 CZ_LSMINFO 标签）")?;
        let lsm = LsmInfo::parse(raw.value.clone())?;
        let ifds = walk_ifds(ifd.offset, size)?;
        if ifds[0].reduced {
            bail!("LSM 文件损坏：第一个图像是缩略图");
        }

        // Full-resolution images: skip the reduced-resolution thumbnails.
        let n_ifds = ifds.len();
        let mut main: Vec<usize> = (0..n_ifds).filter(|&i| !ifds[i].reduced).collect();
        let flagged = main.len() < n_ifds;
        if !flagged && n_ifds >= 2 && n_ifds % 2 == 0 && ifds[1].width < ifds[0].width {
            // Thumbnails without the flag: every other IFD.
            main = (0..n_ifds).step_by(2).collect();
        }
        let paired = n_ifds == main.len() * 2 && main.iter().enumerate().all(|(k, &i)| i == k * 2);

        // Pixel layout, from the first image.
        let get = |t: u16| ifd.uint(tiff, t);
        let w = get(tag::IMAGE_WIDTH).context("LSM 缺少图像宽度")? as u32;
        let h = get(tag::IMAGE_LENGTH).context("LSM 缺少图像高度")? as u32;
        if w == 0 || h == 0 {
            bail!("LSM 图像尺寸为 0");
        }
        let spp = get(tag::SAMPLES_PER_PIXEL).unwrap_or(1).max(1) as u32;
        let planar = get(tag::PLANAR_CONFIG).unwrap_or(1) as u16;
        let channels = (lsm.dim_c().max(1) as u32).min(spp);
        let bits = {
            let e = ifd.get(tag::BITS_PER_SAMPLE).context("LSM 缺少位深")?;
            let mut v = tiff.uints(e);
            // Old writers store two 16-bit values as an offset instead.
            if e.count == 2 && v.first().is_some_and(|b| ![8, 16, 32].contains(b)) {
                let at = u32::from_le_bytes(e.value[..4].try_into().unwrap()) as u64;
                let b = file::read(at, 4)?;
                v = vec![u16::from_le_bytes([b[0], b[1]]) as u64, u16::from_le_bytes([b[2], b[3]]) as u64];
            }
            v
        };
        let used = &bits[..(channels as usize).min(bits.len())];
        let Some(&b0) = used.first() else { bail!("LSM 缺少位深") };
        if used.iter().any(|&b| b != b0) {
            bail!("暂不支持各通道位深不同的 LSM（{}）", used.iter().map(|b| b.to_string()).collect::<Vec<_>>().join("/"));
        }
        let sample_format = get(tag::SAMPLE_FORMAT).unwrap_or(1);
        let dtype = match (b0, sample_format) {
            (8, 2) => DType::I8,
            (8, _) => DType::U8,
            (16, 2) => DType::I16,
            (16, _) => DType::U16,
            (32, 3) => DType::F32,
            (32, _) if lsm.data_type() == 5 => DType::F32,
            (32, 2) => DType::I32,
            (32, _) => DType::U32,
            (b, _) => bail!("不支持 {b} 位的 LSM 图像"),
        };
        let compression = get(tag::COMPRESSION).unwrap_or(1);
        let (codec, predictor) = match compression {
            1 => (Codec::Raw, false),
            5 => (Codec::Lzw, get(tag::PREDICTOR) == Some(2)),
            8 | 32946 => (Codec::Zlib, get(tag::PREDICTOR) == Some(2)),
            32773 => (Codec::PackBits, false),
            c => bail!("不支持的 LSM 压缩方式 {c}"),
        };
        let rows_per_strip = get(tag::ROWS_PER_STRIP).unwrap_or(h as u64).clamp(1, h as u64) as u32;
        let strips = h.div_ceil(rows_per_strip) as usize;
        let geom = Geom { w, h, rows_per_strip, strips, spp, planar, channels, dtype, codec, predictor };

        // Strip offsets of files over 4 GiB wrap around.
        let mut fixed = None;
        if size >= 1 << 32 {
            let load = |l: Option<Loc>| l.and_then(|l| l.read(usize::MAX).ok()).unwrap_or_default();
            let mut offs: Vec<Vec<u64>> = ifds.iter().map(|i| load(i.offsets)).collect();
            let counts: Vec<Vec<u64>> = ifds.iter().map(|i| load(i.counts)).collect();
            // Thumbnail stored before its image? Compared modulo 2^32, as the first pair may straddle the wrap.
            let thumb_first = paired
                && match (offs[0].first(), offs[1].first()) {
                    (Some(&a), Some(&b)) => precedes(b, a),
                    _ => false,
                };
            let mpt = [lsm.dim_m().max(1) as usize, lsm.dim_p().max(1) as usize, lsm.dim_t().max(1) as usize];
            let order = physical_order(n_ifds, paired, thumb_first, mpt);
            unwrap_offsets(&mut offs, &counts, &order);
            fixed = Some(offs);
        }
        let mut me = Lsm { ifds, main, geom, fixed, starts: vec![], base: 0, pages: 0, info: None };
        if codec != Codec::Raw {
            let mut starts: Vec<u64> = Vec::with_capacity(n_ifds);
            starts.extend((0..n_ifds).filter_map(|i| me.first_offset(i)));
            starts.sort_unstable();
            me.starts = starts;
        }

        // Dimensions. Positions and tiles are separate series (tile outermost).
        let (scan_type, size_c) = (lsm.scan_type(), channels);
        let mut size_z = if z_in_stack(scan_type) { lsm.dim_z().max(1) as usize } else { 1 };
        let mut size_t = if t_in_stack(scan_type) { lsm.dim_t().max(1) as usize } else { 1 };
        let (np, nm) = (lsm.dim_p().max(1) as usize, lsm.dim_m().max(1) as usize);
        let mut n_series = np * nm;
        let n_main = me.main.len();
        let mut incomplete = false;
        if n_main < size_z * size_t * n_series {
            incomplete = true;
            if n_series > 1 && n_main >= size_z * size_t {
                n_series = n_main / (size_z * size_t);
            } else {
                n_series = 1;
                if n_main >= size_z {
                    size_t = n_main / size_z;
                } else {
                    size_z = n_main;
                    size_t = 1;
                }
            }
        }
        let per_series = size_z * size_t;
        if series as usize >= n_series {
            bail!("这个 LSM 只有 {n_series} 个图像（请求第 {} 个）", series + 1);
        }
        me.base = series as usize * per_series;
        me.pages = per_series as u32;

        let mut info = Info::new(w, h, dtype, "Zeiss LSM");
        info.size_c = size_c;
        info.size_z = size_z as u32;
        info.size_t = size_t as u32;
        info.samples_per_page = size_c;
        info.page_count = per_series as u32;
        let v = lsm.voxel();
        if v[0] > 0.0 {
            info.pixel_size = Some([v[0] * 1e6, if v[1] > 0.0 { v[1] * 1e6 } else { v[0] * 1e6 }, v[2].max(0.0) * 1e6]);
            info.unit = "µm".into();
        }

        // Scan information and the other blocks.
        let scan = (lsm.off_scan_info() >= 8 && lsm.off_scan_info() < size)
            .then(|| file::read_at_most(lsm.off_scan_info(), MAX_SCAN_INFO).ok())
            .flatten()
            .and_then(|b| parse_scan_info(&b));
        let cc = read_channel_colors(lsm.off_channel_colors());
        let stamps = read_time_stamps(lsm.off_time_stamps());
        let wavelengths = read_f64_records(lsm.off_channel_wavelength(), 2);
        let positions = read_f64_records(lsm.off_positions(), 3);
        let tiles = read_f64_records(lsm.off_tile_positions(), 3);

        for c in 0..size_c as usize {
            let name = cc.names.get(c).filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| format!("C{}", c + 1));
            let fallback = if size_c == 1 { [255, 255, 255] } else { DEFAULT_COLORS[c % 7] };
            let color = cc.colors.get(c).copied().filter(|c| c.iter().any(|&v| v > 0)).unwrap_or(fallback);
            info.channels.push(Channel { name, color });
        }

        if n_series > 1 {
            for s in 0..n_series {
                let (im, ip) = (s / np, s % np);
                let name = match (np > 1, nm > 1) {
                    (true, true) => format!("拼块 {} · 位置 {}", im + 1, ip + 1),
                    (false, true) => format!("拼块 {}", im + 1),
                    _ => format!("位置 {}", ip + 1),
                };
                info.series.push(Series {
                    name,
                    detail: format!("{w}×{h} · {size_c}C · {size_z}Z · {size_t}T"),
                });
            }
            info.series_index = series;
        }
        let place = (n_series > 1).then(|| {
            let (im, ip) = (series as usize / np, series as usize % np);
            let mut items = vec![];
            if np > 1 || nm == 1 {
                if let Some(p) = positions.get(ip) {
                    items.push(("位置", format!("{} / {}（{}，{}，{}）", ip + 1, np, num_f(p[0]), num_f(p[1]), num_f(p[2]))));
                }
            }
            if nm > 1 {
                let at = tiles.get(im).map(|p| format!("（{}，{}，{}）", num_f(p[0]), num_f(p[1]), num_f(p[2]))).unwrap_or_default();
                items.push(("拼块", format!("{} / {}{at}", im + 1, nm)));
            }
            items
        });

        info.metadata = groups(&lsm, scan.as_ref(), &info, &stamps, &wavelengths, place.unwrap_or_default(), incomplete);
        me.info = Some(info);
        Ok(me)
    }
}

impl Format for Lsm {
    fn open(_path: &str) -> Result<Lsm> {
        Lsm::build(0)
    }

    fn open_series(_path: &str, series: u32) -> Result<Lsm> {
        Lsm::build(series)
    }

    fn info(&mut self) -> Result<Info> {
        self.info.take().context("信息已经读取过了")
    }

    fn page(&self, page: u32) -> Result<Recipe> {
        if page >= self.pages {
            bail!("页面 {page} 超出范围");
        }
        let ifd = self.main[self.base + page as usize];
        let g = &self.geom;
        let offs = self.offsets(ifd)?;
        let counts = self.counts(ifd);
        let planar = g.planar == 2 && g.spp > 1;
        let planes = if planar { g.channels as usize } else { 1 };
        if offs.len() < planes * g.strips || (g.codec != Codec::Raw && counts.len() < planes * g.strips) {
            bail!("LSM 页面的条带数量不对（{} 个，需要 {}）", offs.len(), planes * g.strips);
        }
        let samples = if planar { 1 } else { g.spp };
        let row_bytes = g.w as u64 * samples as u64 * g.dtype.size() as u64;

        let mut r = Recipe::new();
        for plane in 0..planes {
            let chunk = |off: u64, len: u64, y: u32, rows: u32| {
                let mut c = Chunk::file(off, len, [0, y, g.w, rows]).samples(samples).plane(plane as u32);
                if g.codec != Codec::Raw {
                    c = c.codec(g.codec);
                    if g.predictor {
                        c = c.flags(flags::PREDICTOR);
                    }
                }
                c
            };
            // Uncompressed strips that follow each other become one chunk, so
            // single-channel pages can be read in place.
            let mut run: Option<(u64, u64, u32, u32)> = None;
            for s in 0..g.strips {
                let k = plane * g.strips + s;
                let y = s as u32 * g.rows_per_strip;
                let rows = g.rows_per_strip.min(g.h - y);
                if g.codec == Codec::Raw {
                    let len = rows as u64 * row_bytes;
                    match run.as_mut() {
                        Some((off, l, _, n)) if *off + *l == offs[k] => {
                            *l += len;
                            *n += rows;
                        }
                        _ => {
                            if let Some((off, l, y0, n)) = run.take() {
                                r.push(chunk(off, l, y0, n));
                            }
                            run = Some((offs[k], len, y, rows));
                        }
                    }
                } else {
                    r.push(chunk(offs[k], self.stored_len(&offs, &counts, k), y, rows));
                }
            }
            if let Some((off, l, y0, n)) = run {
                r.push(chunk(off, l, y0, n));
            }
        }
        Ok(r)
    }
}

// ---------------------------------------------------------------- info-panel groups

/// The detection channels that deliver the image channels, in order: those of
/// the acquired tracks that are switched on. (Track, 1-based ordinal among the
/// acquired tracks, detection channel.)
fn acquired_channels(rec: &Node) -> Vec<(&Node, usize, &Node)> {
    let mut out = vec![];
    let mut ordinal = 0;
    for tr in rec.list(TRACKS) {
        if tr.int(TRACK_ACQUIRE).unwrap_or(0) == 0 {
            continue;
        }
        ordinal += 1;
        for d in tr.list(DET_CHANNELS) {
            if d.int(DET_ACQUIRE).unwrap_or(0) != 0 {
                out.push((tr, ordinal, d));
            }
        }
    }
    out
}

/// Pixel dwell time in µs; newer files store seconds, older ones µs.
fn pixel_time_us(v: f64) -> f64 {
    if v > 0.0 && v < 0.01 {
        v * 1e6
    } else {
        v
    }
}

#[allow(clippy::too_many_arguments)]
fn groups(
    lsm: &LsmInfo,
    scan: Option<&Node>,
    info: &Info,
    stamps: &[f64],
    wavelengths: &[Vec<f64>],
    place: Vec<(&str, String)>,
    incomplete: bool,
) -> Vec<Group> {
    let mut out = vec![];

    // ---- image
    let mut g = Group::new("ZEN black 图像");
    if let Some(rec) = scan {
        g.add("记录名称", rec.text(REC_NAME));
        g.add("描述", rec.text(REC_DESCRIPTION));
        g.add("备注", rec.text(REC_NOTES));
        g.add("用户", rec.text(REC_USER));
        g.add("采集时间", rec.float(REC_SAMPLE0_TIME).and_then(ole_date));
        g.add("扫描模式", rec.text(REC_SCAN_MODE));
    }
    g.add_str("扫描类型", scan_type_name(lsm.scan_type()));
    if lsm.spectral() {
        g.add_str("光谱扫描", "是");
    }
    g.add_str("LSM 结构版本", &format!("{}", lsm.magic() >> 24));
    let mut dims = format!("X={} Y={}", info.width, info.height);
    for (name, n) in [("Z", info.size_z), ("C", info.size_c), ("T", info.size_t)] {
        if n > 1 {
            dims += &format!("  {name}={n}");
        }
    }
    g.add_str("尺寸", &dims);
    g.add_str("像素类型", data_type_name(lsm.data_type()));
    g.add_str("存储类型", info.dtype.name());
    if let Some(p) = info.pixel_size {
        g.add_str("像素尺寸 X", &format!("{:.4} µm", p[0]));
        g.add_str("像素尺寸 Y", &format!("{:.4} µm", p[1]));
        if p[2] > 0.0 {
            g.add_str("Z 步长", &format!("{:.4} µm", p[2]));
        }
    }
    if let Some(rec) = scan {
        g.add("扫描缩放", rec.float(REC_ZOOM_X).filter(|&z| z > 0.0).map(num_f));
        if let (Some(w), Some(h)) = (rec.float(REC_PLANE_WIDTH), rec.float(REC_PLANE_HEIGHT)) {
            if w > 0.0 && h > 0.0 {
                g.add_str("视场", &format!("{} × {} µm", num_f(w), num_f(h)));
            }
        }
    }
    if lsm.time_interval() > 0.0 && info.size_t > 1 {
        g.add_str("时间间隔", &with_unit(lsm.time_interval(), "s"));
    }
    if stamps.len() > 1 {
        let span = stamps[stamps.len() - 1] - stamps[0];
        if span > 0.0 {
            g.add_str("实测总时长", &with_unit(span, "s"));
            g.add_str("实测平均间隔", &with_unit(span / (stamps.len() - 1) as f64, "s"));
        }
    }
    for (k, v) in place {
        g.push(k, &v);
    }
    if incomplete {
        g.add_str("提示", "文件不完整（采集被中断），只显示已保存的图像");
    }
    g.push_to(&mut out);

    // ---- microscope
    if let Some(rec) = scan {
        let mut g = Group::new("ZEN black 显微镜");
        g.add("物镜", rec.text(REC_OBJECTIVE));
        for l in rec.list(LASERS) {
            if let Some(name) = l.text(LASER_NAME) {
                let on = l.int(LASER_ACQUIRE).unwrap_or(0) != 0;
                let power = l.float(LASER_POWER).map(|p| format!("，功率 {}", num_f(p))).unwrap_or_default();
                g.push(&format!("激光器 {name}"), &format!("{}{power}", if on { "启用" } else { "关闭" }));
            }
        }
        let acquired = acquired_channels(rec);
        if let Some((tr, _, _)) = acquired.first() {
            for b in tr.list(BEAM_SPLITTERS) {
                let set = b.text(BS_FILTER_SET).unwrap_or_default();
                if let Some(f) = b.text(BS_FILTER).filter(|f| f != "None") {
                    g.push(&format!("分束器 {set}"), &f);
                }
            }
        }
        g.push_to(&mut out);
    }

    // ---- channels
    let acquired = scan.map(acquired_channels).unwrap_or_default();
    for (i, ch) in info.channels.iter().enumerate() {
        let mut g = Group::new(&format!("通道 {}：{}", i + 1, ch.name));
        g.add_str("颜色", &hex_color(ch.color));
        // Match the channel to its track and detector: by position when the
        // counts agree, else by the "<channel>-T<track>" name ZEN gives it.
        let found = if acquired.len() == info.size_c as usize {
            acquired.get(i).copied()
        } else {
            acquired
                .iter()
                .find(|(_, t, d)| d.text(DET_CHANNEL_NAME).is_some_and(|n| format!("{n}-T{t}") == ch.name))
                .copied()
        };
        if let Some((tr, ordinal, det)) = found {
            g.add("轨道", tr.text(TRACK_NAME).map(|n| format!("{ordinal}：{n}")));
            g.add("检测器", det.text(DET_POINT_DETECTOR));
            g.add("检测通道", det.text(DET_CHANNEL_NAME));
            g.add("染料", det.text(DET_DYE_NAME));
            let filter = [det.text(DET_FILTER_SET), det.text(DET_FILTER)].into_iter().flatten().collect::<Vec<_>>().join(" ");
            g.add_str("滤光片", &filter);
            if det.int(DET_SPECTRAL_CHANNELS).unwrap_or(0) > 0 {
                if let (Some(a), Some(b)) = (det.float(DET_SPI_START), det.float(DET_SPI_STOP)) {
                    g.add_str("光谱范围 (nm)", &format!("{} – {}", num_f(a), num_f(b)));
                }
            }
            g.add("针孔 (µm)", det.float(DET_PINHOLE).filter(|&p| p > 0.0).map(num_f));
            g.add("针孔名称", det.text(DET_PINHOLE_NAME));
            if let Some(first) = det.float(DET_GAIN_FIRST) {
                let last = det.float(DET_GAIN_LAST).unwrap_or(0.0);
                let both = if last != 0.0 && last != first { format!("{} → {}", num_f(first), num_f(last)) } else { num_f(first) };
                g.add_str("检测器增益", &both);
            }
            g.add("放大器增益", det.float(DET_AMP_GAIN_FIRST).map(num_f));
            g.add("放大器偏移", det.float(DET_AMP_OFFS_FIRST).map(num_f));
            for ill in tr.list(ILLUM_CHANNELS) {
                if ill.int(ILLUM_ACQUIRE).unwrap_or(0) != 0 {
                    let wl = ill.float(ILLUM_WAVELENGTH).map(num_f).or(ill.text(ILLUM_NAME)).unwrap_or_default();
                    let power = ill.float(ILLUM_POWER).map(|p| format!("，透过率 {}%", num_f(p))).unwrap_or_default();
                    g.push("激发 (nm)", &format!("{wl}{power}"));
                }
            }
            if let Some(mode) = tr.int(TRACK_SAMPLING_MODE) {
                let n = tr.int(TRACK_SAMPLING_NUMBER).filter(|&n| n > 1 && mode > 0);
                let method = match tr.int(TRACK_SAMPLING_METHOD) {
                    Some(1) => "平均",
                    Some(2) => "求和",
                    _ => "",
                };
                let text = match n {
                    Some(n) => format!("{} × {n}（{method}）", sampling_mode_name(mode)),
                    None => sampling_mode_name(mode).to_string(),
                };
                g.add_str("采样", &text);
            }
            g.add("像素驻留 (µs)", tr.float(TRACK_PIXEL_TIME).map(|v| num_f(pixel_time_us(v))));
            g.add(
                "多路扫描",
                tr.int(TRACK_MULTIPLEX_TYPE).map(|m| if m == 0 { "逐行切换" } else { "逐帧切换" }.to_string()),
            );
            for (name, pos) in [(TRACK_COLLIMATOR1_NAME, TRACK_COLLIMATOR1_POS), (TRACK_COLLIMATOR2_NAME, TRACK_COLLIMATOR2_POS)] {
                if let Some(n) = tr.text(name) {
                    g.add_str("准直器", &format!("{n}（{}）", tr.float(pos).map(num_f).unwrap_or_default()));
                }
            }
            g.add("位深", tr.list(DATA_CHANNELS).iter().find_map(|d| d.int(DATA_BITS)).map(|b| b.to_string()));
        }
        if let Some(wl) = wavelengths.get(i).filter(|w| lsm.spectral() && w[0] > 0.0 && w[1] > 0.0) {
            g.add_str("检测范围 (nm)", &format!("{} – {}", num_f(wl[0]), num_f(wl[1])));
        }
        g.push_to(&mut out);
    }

    // ---- everything, flattened, for search
    let mut g = Group::new("ZEN black 全部参数");
    for (name, v) in lsm.listing() {
        g.push(&format!("LSMINFO/{name}"), &v);
    }
    if let Some(rec) = scan {
        flatten(&mut g, "Recording", rec);
    }
    g.push_to(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_offsets_are_restored_in_physical_order() {
        // Two image/thumbnail pairs; the second image crosses 4 GiB.
        let w = 1u64 << 32;
        let mut offs = vec![vec![w - 300, w - 200], vec![w - 100], vec![50, 150], vec![250]];
        let counts = vec![vec![100, 100], vec![100], vec![100, 100], vec![100]];
        let order = physical_order(4, true, false, [1, 1, 1]);
        assert_eq!(order, [0, 1, 2, 3]);
        unwrap_offsets(&mut offs, &counts, &order);
        assert_eq!(offs, vec![vec![w - 300, w - 200], vec![w - 100], vec![w + 50, w + 150], vec![w + 250]]);
    }

    #[test]
    fn offsets_compare_modulo_4_gib() {
        assert!(precedes(900, 1000));
        assert!(!precedes(1000, 900));
        assert!(!precedes(1000, 1000));
        // The second of a pair that straddles the wrap is still "after".
        assert!(precedes((1 << 32) - 700, 68));
        assert!(!precedes(68, (1 << 32) - 700));
    }

    #[test]
    fn empty_strips_do_not_count_as_wraps() {
        let mut offs = vec![vec![1000, 2000, 0]];
        let counts = vec![vec![1000, 1000, 0]];
        unwrap_offsets(&mut offs, &counts, &[0]);
        assert_eq!(offs, vec![vec![1000, 2000, 0]]);
    }

    #[test]
    fn physical_order_follows_acquisition_when_dimensions_combine() {
        // 2 positions x 2 time points, one pair each: IFD order is p0t0 p0t1 p1t0 p1t1.
        let order = physical_order(8, true, false, [1, 2, 2]);
        // Acquisition order: t0p0 t0p1 t1p0 t1p1.
        assert_eq!(order, [0, 1, 4, 5, 2, 3, 6, 7]);
        // Thumbnails stored first.
        assert_eq!(physical_order(4, true, true, [1, 1, 1]), [1, 0, 3, 2]);
        // Unpaired files keep the IFD order.
        assert_eq!(physical_order(3, false, false, [1, 1, 1]), [0, 1, 2]);
    }

    #[test]
    fn ole_dates() {
        assert_eq!(ole_date(25569.0).as_deref(), Some("1970-01-01 00:00:00"));
        assert_eq!(ole_date(38491.5).as_deref(), Some("2005-05-19 12:00:00"));
        assert_eq!(ole_date(45000.25).as_deref(), Some("2023-03-15 06:00:00"));
        assert_eq!(ole_date(0.0), None);
    }

    #[test]
    fn scan_info_tree() {
        let mut b = vec![];
        let mut rec = |entry: u32, typ: u32, payload: &[u8]| {
            b.extend_from_slice(&entry.to_le_bytes());
            b.extend_from_slice(&typ.to_le_bytes());
            b.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            b.extend_from_slice(payload);
        };
        rec(REC, 0, &[]);
        rec(REC_NAME, 2, b"Scan 1\0");
        rec(TRACKS, 0, &[]);
        rec(TRACK, 0, &[]);
        rec(TRACK_ACQUIRE, 4, &(-1i32).to_le_bytes());
        rec(DET_CHANNELS, 0, &[]);
        rec(DET, 0, &[]);
        rec(DET_PINHOLE, 5, &123.5f64.to_le_bytes());
        rec(0xFFFF_FFFF, 0, &[]);
        rec(0xFFFF_FFFF, 0, &[]);
        rec(0xFFFF_FFFF, 0, &[]);
        rec(0xFFFF_FFFF, 0, &[]);
        rec(0xFFFF_FFFF, 0, &[]);
        let n = parse_scan_info(&b).unwrap();
        assert_eq!(n.text(REC_NAME).as_deref(), Some("Scan 1"));
        let tr = &n.list(TRACKS)[0];
        assert_eq!(tr.int(TRACK_ACQUIRE), Some(-1));
        assert_eq!(tr.list(DET_CHANNELS)[0].float(DET_PINHOLE), Some(123.5));
        assert_eq!(acquired_channels(&n).len(), 0);
    }
}
