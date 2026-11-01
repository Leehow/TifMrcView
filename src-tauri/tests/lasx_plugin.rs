//! Leica LAS X (LIF / LOF) through the "lasx" WebAssembly plugin, end to end:
//! the plugin is built from `plugins/lasx`, installed into a temporary
//! plugin folder and used through `formats::open_with` /
//! `formats::open_series_with`, exactly as the app does.

mod common;

use std::path::Path;

use common::lasx::{self, Img, Node};
use tifmrcview_lib::formats;
use tifmrcview_lib::image::{DType, ImageInfo, PlaneData, Source};
use tifmrcview_lib::plugins::registry::Registry;

macro_rules! lasx {
    () => {
        match common::install(&["lasx"]) {
            Some(mut p) => {
                // The plugin needs 0.5.0 (series); the crate may still say 0.4.x.
                p.registry = Registry::new(Some(p.dir.path().to_path_buf()), "0.5.0");
                p
            }
            None => return,
        }
    };
}

fn values(p: &PlaneData) -> Vec<f64> {
    (0..p.len()).map(|i| p.get_f64(i)).collect()
}

fn open(reg: &Registry, path: &Path, series: usize) -> Box<dyn Source> {
    formats::open_series_with(path, series, reg)
        .unwrap_or_else(|e| panic!("{}: {e:#}", path.display()))
}

fn err(reg: &Registry, path: &Path, series: usize) -> String {
    match formats::open_series_with(path, series, reg) {
        Ok(_) => panic!("{} opened", path.display()),
        Err(e) => format!("{e:#}"),
    }
}

/// The samples of plane (c, z, t).
fn plane(src: &dyn Source, c: usize, z: usize, t: usize) -> Vec<f64> {
    let (page, s) = src.info().locate(c, z, t);
    values(&src.read_page(page).unwrap()[s].data)
}

fn item(i: &ImageInfo, group: &str, label: &str) -> Option<String> {
    i.metadata
        .iter()
        .find(|g| g.name == group)?
        .items
        .iter()
        .find(|it| it[0] == label)
        .map(|it| it[1].clone())
}

fn group_names(i: &ImageInfo) -> Vec<&str> {
    i.metadata.iter().map(|g| g.name.as_str()).collect()
}

fn pattern_plane(w: usize, h: usize, c: usize, z: usize, t: usize, mask: u16) -> Vec<f64> {
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (lasx::pattern(c, z, t, x, y) & mask) as f64))
        .collect()
}

#[test]
fn two_series_stack_info_and_pixels() {
    let p = lasx!();
    let tmp = tempfile::tempdir().unwrap();
    // Version 1 files have 32-bit block sizes, version 2 64-bit ones.
    for version in [1, 2] {
        let path = tmp.path().join(format!("v{version}.lif"));
        let nodes = vec![
            Node::Image(lasx::stack("Stack A", 40, 30, 2, 3, 1, 16)),
            Node::Folder("Empty".into(), vec![]),
            Node::Folder(
                "Sub".into(),
                vec![Node::Image(lasx::stack("Series002", 20, 10, 1, 1, 3, 8))],
            ),
        ];
        lasx::write_lif(&path, version, "Project", &nodes);

        let src = open(&p.registry, &path, 0);
        let i = src.info();
        assert_eq!(
            (
                i.width,
                i.height,
                i.size_c,
                i.size_z,
                i.size_t,
                i.page_count
            ),
            (40, 30, 2, 3, 1, 6)
        );
        assert_eq!(i.dtype, DType::U16);
        assert_eq!(i.format, "Leica LIF");
        assert_eq!(i.unit, "µm");
        let ps = i.pixel_size.unwrap();
        assert!(
            (ps[0] - 0.5).abs() < 1e-9 && (ps[1] - 0.5).abs() < 1e-9 && (ps[2] - 1.5).abs() < 1e-9,
            "{ps:?}"
        );
        assert_eq!(
            i.channels.iter().map(|c| c.color).collect::<Vec<_>>(),
            [[0, 255, 0], [255, 0, 0]]
        );
        // Folders and the root are not series; names are tree paths.
        let names: Vec<_> = i.series.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Stack A", "Sub/Series002"]);
        assert_eq!(i.series[0].detail, "40×30 · 2C · 3Z · 16 bit");
        assert_eq!(i.series[1].detail, "20×10 · 3T · 8 bit");
        assert_eq!(i.series_index, 0);
        assert_eq!(group_names(i), ["LAS X 图像", "LAS X 全部参数"]);
        assert_eq!(item(i, "LAS X 图像", "图像").as_deref(), Some("Stack A"));
        assert_eq!(
            item(i, "LAS X 图像", "采集时间").as_deref(),
            Some("2017-01-30 07:56:07 UTC")
        );
        assert_eq!(
            item(i, "LAS X 图像", "像素尺寸 X").as_deref(),
            Some("0.5000 µm")
        );
        assert_eq!(
            item(i, "LAS X 图像", "Z 步长").as_deref(),
            Some("1.5000 µm")
        );
        assert_eq!(
            item(i, "LAS X 图像", "尺寸").as_deref(),
            Some("X=40  Y=30  C=2  Z=3")
        );

        for z in 0..3 {
            for c in 0..2 {
                assert_eq!(
                    plane(&*src, c, z, 0),
                    pattern_plane(40, 30, c, z, 0, u16::MAX),
                    "c{c} z{z}"
                );
                // Uncompressed single-chunk pages can be read in place (side views).
                let (page, _) = i.locate(c, z, 0);
                let (raw, little) = src.raw_page(page).unwrap();
                assert!(little);
                let want: Vec<u8> = pattern_plane(40, 30, c, z, 0, u16::MAX)
                    .iter()
                    .flat_map(|&v| (v as u16).to_le_bytes())
                    .collect();
                assert_eq!(raw, &want[..]);
            }
        }

        // The second image of the project.
        let src = open(&p.registry, &path, 1);
        let i = src.info();
        assert_eq!(
            (
                i.width,
                i.height,
                i.size_c,
                i.size_z,
                i.size_t,
                i.page_count
            ),
            (20, 10, 1, 1, 3, 3)
        );
        assert_eq!(i.dtype, DType::U8);
        assert_eq!(i.series_index, 1);
        assert_eq!(i.series.len(), 2);
        assert_eq!(i.channels[0].color, [0, 255, 0]);
        assert_eq!(
            item(i, "LAS X 图像", "图像").as_deref(),
            Some("Sub/Series002")
        );
        assert_eq!(item(i, "LAS X 图像", "时间间隔").as_deref(), Some("2.5 s"));
        for t in 0..3 {
            assert_eq!(
                plane(&*src, 0, 0, t),
                pattern_plane(20, 10, 0, 0, t, 0xff),
                "t{t}"
            );
        }
        assert!(err(&p.registry, &path, 2).contains("没有这个图像"));
    }
}

#[test]
fn lof_single_image() {
    let p = lasx!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("one.lof");
    lasx::write_lof(&path, &lasx::stack("Series001", 24, 16, 2, 1, 4, 16));
    let src = open(&p.registry, &path, 0);
    let i = src.info();
    assert_eq!(
        (i.width, i.height, i.size_c, i.size_z, i.size_t),
        (24, 16, 2, 1, 4)
    );
    assert_eq!(i.format, "Leica LOF");
    assert!(i.series.len() <= 1);
    assert_eq!(
        item(i, "LAS X 图像", "文件类型").as_deref(),
        Some("Leica LOF")
    );
    for t in 0..4 {
        for c in 0..2 {
            assert_eq!(
                plane(&*src, c, 0, t),
                pattern_plane(24, 16, c, 0, t, u16::MAX)
            );
        }
    }
    assert!(err(&p.registry, &path, 1).contains("没有这个图像"));
}

/// Pixel-interleaved colour images: BGR order, padded rows, and other channel counts.
#[test]
fn interleaved_rgb_and_samples() {
    let p = lasx!();
    let tmp = tempfile::tempdir().unwrap();
    let (w, h, pad) = (5usize, 3usize, 4usize);
    let rgb = |x: usize, y: usize| [(10 + x + 20 * y) as u8, (100 + x) as u8, (200 + y) as u8];
    let make = |bgr: bool| {
        let mut data = vec![];
        for y in 0..h {
            for x in 0..w {
                let [r, g, b] = rgb(x, y);
                data.extend_from_slice(&if bgr { [b, g, r] } else { [r, g, b] });
            }
            data.extend_from_slice(&vec![0xEE; pad]);
        }
        let tags = if bgr { [3, 2, 1] } else { [1, 2, 3] };
        let luts = if bgr {
            ["Blue", "Green", "Red"]
        } else {
            ["Red", "Green", "Blue"]
        };
        Img {
            name: "Photo".into(),
            dims: vec![
                (1, w as u64, 0.0, 4e-6, "m", 3),
                (2, h as u64, 0.0, 2e-6, "m", (3 * w + pad) as u64),
            ],
            chans: (0..3).map(|k| (8, luts[k], k as u64, tags[k])).collect(),
            data,
            attachments: String::new(),
            chan_props: vec![],
        }
    };
    for bgr in [false, true] {
        let path = tmp.path().join(format!("rgb{bgr}.lof"));
        lasx::write_lof(&path, &make(bgr));
        let src = open(&p.registry, &path, 0);
        let i = src.info();
        assert!(i.is_rgb, "bgr={bgr}");
        assert_eq!(
            (i.size_c, i.samples_per_page, i.page_count, i.dtype),
            (3, 3, 1, DType::U8)
        );
        assert_eq!(i.series.len(), 0);
        let planes = src.read_page(0).unwrap();
        assert_eq!(planes.len(), 3);
        for (k, pl) in planes.iter().enumerate() {
            let want: Vec<f64> = (0..h)
                .flat_map(|y| (0..w).map(move |x| rgb(x, y)[k] as f64))
                .collect();
            assert_eq!(values(&pl.data), want, "bgr={bgr} sample {k}");
        }
    }

    // Two interleaved 16-bit channels are channels, not colour.
    let mut data = vec![];
    for y in 0..3usize {
        for x in 0..4usize {
            data.extend_from_slice(&(lasx::pattern(0, 0, 0, x, y)).to_le_bytes());
            data.extend_from_slice(&(lasx::pattern(1, 0, 0, x, y)).to_le_bytes());
        }
    }
    let img = Img {
        name: "Dual".into(),
        dims: vec![(1, 4, 0.0, 3e-6, "m", 4), (2, 3, 0.0, 2e-6, "m", 16)],
        chans: vec![(16, "Green", 0, 0), (16, "Magenta", 2, 0)],
        data,
        attachments: String::new(),
        chan_props: vec![],
    };
    let path = tmp.path().join("dual.lof");
    lasx::write_lof(&path, &img);
    let src = open(&p.registry, &path, 0);
    let i = src.info();
    assert!(!i.is_rgb);
    assert_eq!(
        (i.size_c, i.samples_per_page, i.page_count, i.dtype),
        (2, 2, 1, DType::U16)
    );
    assert_eq!(i.channels[1].color, [255, 0, 255]);
    let planes = src.read_page(0).unwrap();
    for c in 0..2 {
        assert_eq!(
            values(&planes[c].data),
            pattern_plane(4, 3, c, 0, 0, u16::MAX)
        );
    }
}

fn tile_image(len_is_n_steps: bool, attachments: &str) -> Img {
    // Four 8x8 tiles, tile m filled with m + 1; 1 µm pixels.
    let (w, nt) = (8usize, 4usize);
    let length = if len_is_n_steps {
        w as f64 * 1e-6
    } else {
        (w as f64 - 1.0) * 1e-6
    };
    let mut data = vec![];
    for m in 0..nt {
        data.extend(std::iter::repeat((m + 1) as u8).take(w * w));
    }
    Img {
        name: "Mosaic".into(),
        dims: vec![
            (1, w as u64, 0.0, length, "m", 1),
            (2, w as u64, 0.0, length, "m", w as u64),
            (10, nt as u64, 0.0, 0.0, "", (w * w) as u64),
        ],
        chans: vec![(8, "Gray", 0, 0)],
        data,
        attachments: attachments.into(),
        chan_props: vec![],
    }
}

fn tile_scan_info(swap: u8) -> String {
    // Pitch of 6 pixels (2 px overlap): tiles at (0,0) (6,0) (0,6) (6,6) in µm.
    let mut x =
        format!("<Attachment Name=\"TileScanInfo\" FlipX=\"0\" FlipY=\"0\" SwapXY=\"{swap}\">");
    for (fx, fy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        x.push_str(&format!(
            "<Tile FieldX=\"{fx}\" FieldY=\"{fy}\" PosX=\"{:e}\" PosY=\"{:e}\" PosZ=\"0\"/>",
            1e-3 + fx as f64 * 6e-6,
            2e-3 + fy as f64 * 6e-6
        ));
    }
    x.push_str("</Attachment>");
    x
}

#[test]
fn tile_scan_is_stitched_by_position() {
    let p = lasx!();
    let tmp = tempfile::tempdir().unwrap();
    // Length can be written as n-1 or as n pixel steps; both land on whole pixels.
    for (name, n_steps) in [("nm1", false), ("n", true)] {
        let path = tmp.path().join(format!("{name}.lof"));
        lasx::write_lof(&path, &tile_image(n_steps, &tile_scan_info(0)));
        let src = open(&p.registry, &path, 0);
        let i = src.info();
        assert_eq!((i.width, i.height, i.page_count), (14, 14, 1), "{name}");
        assert_eq!(i.series.len(), 0);
        assert!(item(i, "LAS X 图像", "拼块").unwrap().contains("14×14"));
        let mut want = vec![0.0; 14 * 14];
        for (m, (ox, oy)) in [(0, 0), (6, 0), (0, 6), (6, 6)].into_iter().enumerate() {
            for y in 0..8 {
                for x in 0..8 {
                    want[(oy + y) * 14 + ox + x] = (m + 1) as f64; // later tiles overwrite
                }
            }
        }
        assert_eq!(plane(&*src, 0, 0, 0), want, "{name}");
    }

    // No positions, or a swapped scan: tile 0 only.
    for (name, att) in [("nopos", String::new()), ("swap", tile_scan_info(1))] {
        let path = tmp.path().join(format!("{name}.lof"));
        lasx::write_lof(&path, &tile_image(false, &att));
        let src = open(&p.registry, &path, 0);
        let i = src.info();
        assert_eq!((i.width, i.height), (8, 8), "{name}");
        assert_eq!(plane(&*src, 0, 0, 0), vec![1.0; 64]);
        assert!(item(i, "LAS X 图像", "拼块")
            .unwrap()
            .contains("只显示第 1 个"));
    }
}

const HARDWARE: &str = r#"<Attachment Name="HardwareSetting" Application="LAS AF" Software="LAS X 4.3.0.24308" SystemTypeName="STELLARIS 5" DataSourceTypeName="Confocal">
<ATLConfocalSettingDefinition MicroscopeModel="DMI8-CS" ObjectiveName="HC PL APO CS2    63x/1.40 OIL " Magnification="63" NumericalAperture="1.4" Immersion="OIL" Zoom="2.25109702165419" ScanSpeed="400" ScanDirectionXName="Unidirectional" Pinhole="9.54070473876063E-05" PinholeAiry="0.998754761924568" LineAverage="2" FrameAverage="1" PixelDwellTime="0.0000031625" InDimension="40" OutDimension="30">
<Spectro><MultiBand Channel="2" LeftWorld="430" RightWorld="488.999999"/><MultiBand Channel="3" LeftWorld="502" RightWorld="551.595584643939"/></Spectro>
<AotfList><Aotf LightSourceName="UV Light"><BeamRoute Version="0"/><LaserLineSetting LaserLine="405" IntensityDev="1.84467810367091E-02" IsVisible="1"/></Aotf>
<Aotf LightSourceName="Visible Light"><LaserLineSetting LaserLine="488" IntensityDev="0" IsVisible="1"/><LaserLineSetting LaserLine="561" IntensityDev="12.5" IsVisible="0"/></Aotf></AotfList>
<DetectorList><Detector Name="HyD S 1" Type="SiPM" Channel="1" IsActive="0" Gain="100"/>
<Detector Name="HyD S 2" Type="SiPM" Channel="2" IsActive="1" DyeName="Leica/ALEXA 405" Gain="9.107421875" Offset="0" AcquisitionModeName="PhotonCounting"/>
<Detector Name="HyD S 3" Type="HyD" Channel="3" IsActive="1" Gain="2.888671875" Offset="-0.0066666666"/></DetectorList>
</ATLConfocalSettingDefinition></Attachment>"#;

#[test]
fn hardware_settings_become_groups() {
    let p = lasx!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("hw.lof");
    let mut img = lasx::stack("Series001", 40, 30, 2, 1, 1, 16);
    img.attachments = HARDWARE.into();
    // Channel 1 says which detector and dye it used; channel 2 only the detector (the dye comes from it).
    img.chan_props = vec![
        vec![("DetectorName", "HyD S 3"), ("DyeName", "Leica/EGFP")],
        vec![("DetectorName", "HyD S 2"), ("DyeName", "")],
    ];
    lasx::write_lof(&path, &img);
    let src = open(&p.registry, &path, 0);
    let i = src.info();
    assert_eq!(i.format, "Leica LOF (LAS X 4.3.0.24308)");
    assert_eq!(
        group_names(i),
        [
            "LAS X 图像",
            "LAS X 显微镜",
            "通道 1：EGFP",
            "通道 2：ALEXA 405",
            "LAS X 全部参数"
        ]
    );
    assert_eq!(
        i.channels
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["EGFP", "ALEXA 405"]
    );
    assert_eq!(
        item(i, "LAS X 图像", "软件").as_deref(),
        Some("LAS X 4.3.0.24308")
    );
    assert_eq!(
        item(i, "LAS X 图像", "系统").as_deref(),
        Some("STELLARIS 5")
    );
    let m = |k: &str| item(i, "LAS X 显微镜", k);
    assert_eq!(m("物镜").as_deref(), Some("HC PL APO CS2 63x/1.40 OIL"));
    assert_eq!(m("数值孔径 NA").as_deref(), Some("1.4"));
    assert_eq!(m("浸没介质").as_deref(), Some("OIL"));
    assert_eq!(m("变焦").as_deref(), Some("2.2511"));
    assert_eq!(m("针孔 (µm)").as_deref(), Some("95.407"));
    assert_eq!(m("针孔 (AU)").as_deref(), Some("0.9988"));
    assert_eq!(m("行平均").as_deref(), Some("2"));
    assert_eq!(m("像素驻留 (µs)").as_deref(), Some("3.1625"));
    assert_eq!(m("扫描格式").as_deref(), Some("40×30"));
    let c1 = |k: &str| item(i, "通道 1：EGFP", k);
    assert_eq!(c1("探测器").as_deref(), Some("HyD S 3 (HyD)"));
    assert_eq!(c1("增益").as_deref(), Some("2.8887"));
    assert_eq!(c1("偏移").as_deref(), Some("-0.0067"));
    assert_eq!(c1("检测范围 (nm)").as_deref(), Some("502 – 551.5956"));
    // Only lines that are on with a non-zero intensity count.
    assert_eq!(c1("激光").as_deref(), Some("405 nm, 0.0184%"));
    let c2 = |k: &str| item(i, "通道 2：ALEXA 405", k);
    assert_eq!(c2("探测器").as_deref(), Some("HyD S 2 (SiPM)"));
    assert_eq!(c2("采集模式").as_deref(), Some("PhotonCounting"));
    assert_eq!(c2("检测范围 (nm)").as_deref(), Some("430 – 489"));

    // Every attribute and text of the image, with paths.
    let all = |k: &str| item(i, "LAS X 全部参数", k);
    assert_eq!(
        all("Image/Attachment[HardwareSetting]/ATLConfocalSettingDefinition@Zoom").as_deref(),
        Some("2.25109702165419")
    );
    assert_eq!(all("Image/Attachment[HardwareSetting]/ATLConfocalSettingDefinition/DetectorList/Detector[HyD S 3]@Gain").as_deref(), Some("2.888671875"));
    assert_eq!(
        all("Image/ImageDescription/Channels/ChannelDescription/ChannelProperty[DetectorName]")
            .as_deref(),
        Some("HyD S 3")
    );
    assert_eq!(
        all("Image/ImageDescription/Channels/ChannelDescription#2@LUTName").as_deref(),
        Some("Red")
    );
    assert_eq!(
        all("Image/ImageDescription/Dimensions/DimensionDescription#2@NumberOfElements").as_deref(),
        Some("30")
    );
    // Only this image: nothing from the container's root.
    assert!(i
        .metadata
        .last()
        .unwrap()
        .items
        .iter()
        .all(|it| !it[0].contains("Experiment")));
}

/// The flattened parameters belong to the selected image, not the first one.
#[test]
fn all_settings_follow_the_selected_series() {
    let p = lasx!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("two.lif");
    let mut a = lasx::stack("A", 8, 8, 1, 1, 1, 8);
    a.attachments = "<Attachment Name=\"Marker\" Which=\"first\"/>".into();
    let mut b = lasx::stack("B", 9, 9, 1, 1, 1, 8);
    b.attachments = "<Attachment Name=\"Marker\" Which=\"second\"/>".into();
    lasx::write_lif(
        &path,
        2,
        "Root",
        &[
            Node::Image(a),
            Node::Folder("F".into(), vec![Node::Image(b)]),
        ],
    );
    for (series, which) in [(0, "first"), (1, "second")] {
        let src = open(&p.registry, &path, series);
        assert_eq!(
            item(
                src.info(),
                "LAS X 全部参数",
                "Image/Attachment[Marker]@Which"
            )
            .as_deref(),
            Some(which)
        );
    }
}

#[test]
fn stopped_scans_and_broken_files() {
    let p = lasx!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("cut.lif");
    lasx::write_lif(
        &path,
        2,
        "Project",
        &[Node::Image(lasx::stack("S", 16, 16, 2, 3, 1, 16))],
    );
    let full = std::fs::read(&path).unwrap();

    // Each case gets its own file: Windows cannot rewrite a file that is still memory-mapped.
    let mut n = 0;
    let mut fresh = |bytes: &[u8]| {
        n += 1;
        let p = tmp.path().join(format!("case{n}.lif"));
        std::fs::write(&p, bytes).unwrap();
        p
    };

    // The last plane is missing (acquisition stopped): the others still read, it stays blank.
    let path = fresh(&full[..full.len() - 100]);
    let src = open(&p.registry, &path, 0);
    assert_eq!(
        plane(&*src, 0, 0, 0),
        pattern_plane(16, 16, 0, 0, 0, u16::MAX)
    );
    assert_eq!(plane(&*src, 1, 2, 0), vec![0.0; 256]);
    assert_eq!(
        plane(&*src, 0, 2, 0),
        pattern_plane(16, 16, 0, 2, 0, u16::MAX)
    );

    // Garbage and cuts anywhere are errors, never crashes.
    for cut in [0, 5, 12, 40, 400, 1500, full.len() / 2] {
        let path = fresh(&full[..cut]);
        let _ = formats::open_series_with(&path, 0, &p.registry).and_then(|s| s.read_page(0));
    }
    let path = fresh(b"this is not a Leica file at all");
    assert!(err(&p.registry, &path, 0).contains("不是徕卡"));
    let path = fresh(b"");
    assert!(!err(&p.registry, &path, 0).is_empty());

    // A project whose block is missing from the file.
    let path = tmp.path().join("small.lif");
    lasx::write_lif(
        &path,
        2,
        "Project",
        &[Node::Image(lasx::stack("S", 4, 4, 1, 1, 1, 8))],
    );
    let mut bytes = std::fs::read(&path).unwrap();
    let xml_end = 13 + 2 * u32::from_le_bytes(bytes[9..13].try_into().unwrap()) as usize;
    bytes.truncate(xml_end);
    let path = fresh(&bytes);
    assert!(err(&p.registry, &path, 0).contains("找不到图像数据块"));

    // Unsupported pixel type, and a layout that is not row-major.
    let mut bad = lasx::stack("S", 4, 4, 1, 1, 1, 8);
    bad.chans[0].0 = 64;
    let path = tmp.path().join("bad1.lof");
    lasx::write_lof(&path, &bad);
    assert!(err(&p.registry, &path, 0).contains("像素类型"));
    let mut bad = lasx::stack("S", 4, 4, 1, 1, 1, 8);
    bad.dims[1].5 = 2;
    let path = tmp.path().join("bad2.lof");
    lasx::write_lof(&path, &bad);
    assert!(err(&p.registry, &path, 0).contains("不支持的 LIF 像素排列"));
}

#[test]
fn needs_the_plugin_and_follows_the_switch() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.lif");
    lasx::write_lif(
        &path,
        2,
        "P",
        &[Node::Image(lasx::stack("S", 4, 4, 1, 1, 1, 8))],
    );
    let none = Registry::new(Some(tmp.path().join("no-plugins")), "0.5.0");
    // (The host's list of well-known plugins is what turns this into "install the plugin".)
    assert!(err(&none, &path, 0).contains(".lif"));
    assert!(!formats::is_supported_with(&path, &none));

    let p = lasx!();
    assert!(formats::is_supported_with(&path, &p.registry));
    let mut ext = p.registry.extensions();
    ext.sort();
    assert_eq!(ext, ["lif", "lof"]);
    p.registry.set_disabled(vec!["lasx".into()]).unwrap();
    assert!(err(&p.registry, &path, 0).contains("已停用"));
    p.registry.set_disabled(vec![]).unwrap();
    assert!(formats::open_with(&path, &p.registry).is_ok());
}

#[test]
fn large_projects_stay_within_the_fuel_budget() {
    let p = lasx!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("big.lif");
    // 400 images and a few MB of settings: more than most real projects.
    let mut nodes = vec![];
    for k in 0..400 {
        let mut img = lasx::stack(&format!("Series{k:03}"), 8, 8, 2, 2, 1, 16);
        let mut att = String::from(
            "<Attachment Name=\"HardwareSetting\"><ATLConfocalSettingDefinition Zoom=\"1\">",
        );
        for j in 0..40 {
            att.push_str(&format!(
                "<Detector Name=\"D{j}\" Gain=\"{j}\" Offset=\"0\" IsActive=\"0\"/>"
            ));
        }
        att.push_str("</ATLConfocalSettingDefinition></Attachment>");
        img.attachments = att;
        nodes.push(Node::Folder(
            format!("Pos{}", k / 20),
            vec![Node::Image(img)],
        ));
    }
    lasx::write_lif(&path, 2, "Experiment", &nodes);
    let kb = std::fs::metadata(&path).unwrap().len() >> 10;
    for series in [0, 399] {
        let t = std::time::Instant::now();
        let src = open(&p.registry, &path, series);
        eprintln!(
            "opened series {series} of a {kb} KB LIF with 400 images in {:?}",
            t.elapsed()
        );
        let i = src.info();
        assert_eq!(i.series.len(), 400);
        assert_eq!(i.series[399].name, "Pos19/Series399");
        assert_eq!(
            plane(&*src, 1, 1, 0),
            pattern_plane(8, 8, 1, 1, 0, u16::MAX)
        );
        assert_eq!(
            item(
                i,
                "LAS X 全部参数",
                "Image/Attachment[HardwareSetting]/ATLConfocalSettingDefinition/Detector[D39]@Gain"
            )
            .as_deref(),
            Some("39")
        );
    }
}

// ------------------------------------------------------------------ real files (skipped when absent)

fn sample(rel: &str) -> Option<std::path::PathBuf> {
    let p = Path::new("/home/user/samples").join(rel);
    p.exists().then_some(p)
}

fn sum(v: &[f64]) -> u64 {
    v.iter().sum::<f64>() as u64
}

/// (series, name, width, c, z, t, sum of first plane, sum of last plane); the
/// sums come from liffile (BSD-3), which reads the same planes.
const FRET: [(usize, &str, usize, usize, usize, usize, u64, u64); 8] = [
    (0, "423_001", 1024, 2, 1, 1, 11614995, 104804559),
    (1, "423_002", 1024, 2, 1, 1, 32178927, 28640963),
    (
        2,
        "FRET_AB_423_001/DonAcc_PreBleach",
        1024,
        2,
        1,
        1,
        79156057,
        28436101,
    ),
    (
        3,
        "FRET_AB_423_001/Bleach003",
        1024,
        1,
        10,
        1,
        133077090,
        113574953,
    ),
    (
        4,
        "FRET_AB_423_001/DonAcc_PostBleach",
        1024,
        2,
        1,
        1,
        88000562,
        25550033,
    ),
    (
        5,
        "FRET_AB_423_001/FRET-Efficiency",
        1024,
        1,
        1,
        1,
        35971946,
        35971946,
    ),
    (6, "Series005", 512, 2, 16, 1, 378788, 1526010),
    (7, "Series006", 512, 2, 39, 1, 378080, 1926425),
];

#[test]
fn sample_lif_series_match_liffile() {
    let Some(path) = sample("lif/20191025 Test FRET 585. 423, 426.lif") else {
        return;
    };
    let p = lasx!();
    for &(series, name, w, c, z, t, first, last) in &FRET {
        let t0 = std::time::Instant::now();
        let src = open(&p.registry, &path, series);
        let open_time = t0.elapsed();
        let i = src.info();
        assert_eq!(i.series.len(), 8);
        assert_eq!(i.series[series].name, name);
        assert_eq!(i.series_index, series);
        assert_eq!(
            (i.width, i.height, i.size_c, i.size_z, i.size_t, i.dtype),
            (w, w, c, z, t, DType::U8),
            "{name}"
        );
        assert_eq!(sum(&plane(&*src, 0, 0, 0)), first, "{name} first plane");
        assert_eq!(
            sum(&plane(&*src, c - 1, z - 1, t - 1)),
            last,
            "{name} last plane"
        );
        let ps = i.pixel_size.unwrap();
        assert!(
            ps[0] > 0.05 && ps[0] < 0.2 && (ps[0] - ps[1]).abs() < 1e-9,
            "{name}: {ps:?}"
        );
        // The computed FRET-Efficiency image has no hardware settings of its own.
        if series != 5 {
            assert_eq!(
                item(i, "LAS X 图像", "软件").as_deref(),
                Some("LAS X 3.5.2.18963"),
                "{name}"
            );
        }
        eprintln!(
            "{name}: {}  opened in {open_time:?}, groups {:?}",
            i.format,
            group_names(i)
        );
    }
    // The first series: pixel size 1.37e-20.. → Length/(N-1) in µm, cyan and red LUTs, detector info.
    let src = open(&p.registry, &path, 0);
    let i = src.info();
    assert_eq!(
        i.channels.iter().map(|c| c.color).collect::<Vec<_>>(),
        [[0, 255, 255], [255, 0, 0]]
    );
    assert_eq!(
        item(i, "LAS X 显微镜", "数值孔径 NA").as_deref(),
        Some("1.4")
    );
    assert_eq!(
        item(i, "LAS X 显微镜", "物镜").as_deref(),
        Some("HC PL APO CS2 63x/1.40 OIL")
    );
    assert_eq!(item(i, "LAS X 图像", "采集时间").is_some(), true);
    // Six-image Z stack: Z step from the Z dimension.
    let src = open(&p.registry, &path, 6);
    assert!(src.info().pixel_size.unwrap()[2] > 0.0);
}

#[test]
fn sample_lif_tile_scan_is_stitched() {
    let Some(path) = sample("lif/PR2729_frameOrderCombinedScanTypes.lif") else {
        return;
    };
    let p = lasx!();
    let src = open(&p.registry, &path, 0);
    let i = src.info();
    // 2 T x 3 Z x 2 C, four 64x64 tiles at a 34 pixel pitch.
    assert_eq!(
        (
            i.width,
            i.height,
            i.size_c,
            i.size_z,
            i.size_t,
            i.page_count
        ),
        (98, 98, 2, 3, 2, 12)
    );
    assert!(item(i, "LAS X 图像", "拼块").unwrap().contains("98×98"));
    // Composites built with numpy from liffile's frames and tile positions.
    assert_eq!(sum(&plane(&*src, 0, 0, 0)), 570372);
    assert_eq!(sum(&plane(&*src, 1, 2, 1)), 594390);
    assert_eq!(sum(&plane(&*src, 1, 1, 0)), 663734);
}

#[test]
fn sample_lof_matches_liffile() {
    let Some(path) = sample("lof/xyct 16bit 2ch t11.lof") else {
        return;
    };
    let p = lasx!();
    let t0 = std::time::Instant::now();
    let src = open(&p.registry, &path, 0);
    let i = src.info();
    eprintln!(
        "LOF opened in {:?}; groups {:?}",
        t0.elapsed(),
        group_names(i)
    );
    assert_eq!(
        (i.width, i.height, i.size_c, i.size_z, i.size_t, i.dtype),
        (512, 512, 2, 1, 11, DType::U16)
    );
    assert_eq!(i.format, "Leica LOF (LAS X 4.3.0.24308)");
    assert_eq!(sum(&plane(&*src, 0, 0, 0)), 1890797909);
    assert_eq!(sum(&plane(&*src, 1, 0, 10)), 418168306);
    let ps = i.pixel_size.unwrap();
    assert!((ps[0] - 8.197062e-5 * 1e6 / 511.0).abs() < 1e-6, "{ps:?}");
    assert_eq!(
        item(i, "LAS X 显微镜", "数值孔径 NA").as_deref(),
        Some("1.4")
    );
    assert_eq!(item(i, "LAS X 显微镜", "变焦").as_deref(), Some("2.2511"));
    assert_eq!(
        item(i, "LAS X 图像", "时间间隔").as_deref(),
        Some("1.422 s")
    );
    // Both channels have a detector, dye and detection range.
    let ch: Vec<&str> = group_names(i)
        .into_iter()
        .filter(|n| n.starts_with("通道"))
        .collect();
    assert_eq!(ch.len(), 2, "{ch:?}");
    assert!(i.metadata.last().unwrap().items.len() > 300);
}
