//! Writes small synthetic Leica LIF and LOF files for tests.

use std::path::Path;

/// One dimension: id, elements, origin, length, unit, byte stride.
pub type Dim = (u32, u64, f64, f64, &'static str, u64);
/// One channel: bits, LUT name, byte stride, channel tag.
pub type Chan = (u32, &'static str, u64, u32);

/// An image element: dimensions, channels and the raw pixels.
pub struct Img {
    pub name: String,
    pub dims: Vec<Dim>,
    pub chans: Vec<Chan>,
    pub data: Vec<u8>,
    /// Extra XML inside `<Image>`, before the description (attachments).
    pub attachments: String,
    /// Key/value `ChannelProperty` entries of each channel (may be shorter than `chans`).
    pub chan_props: Vec<Vec<(&'static str, &'static str)>>,
}

pub enum Node {
    Folder(String, Vec<Node>),
    Image(Img),
}

/// Value of the test pattern at channel `c`, plane `z`, time `t`, pixel (x, y).
pub fn pattern(c: usize, z: usize, t: usize, x: usize, y: usize) -> u16 {
    (c * 4000 + z * 500 + t * 60 + x + 7 * y) as u16
}

/// A planar 16-bit (or 8-bit when `bits` is 8) stack, channels innermost
/// (like LAS X: X, Y, then channel blocks inside each Z step, T slowest).
pub fn stack(name: &str, w: usize, h: usize, nc: usize, nz: usize, nt: usize, bits: u32) -> Img {
    let item = if bits > 8 { 2 } else { 1 };
    let plane = (w * h * item) as u64;
    let mut dims: Vec<Dim> = vec![
        (1, w as u64, 0.0, (w as f64 - 1.0) * 0.5e-6, "m", item as u64),
        (2, h as u64, 0.0, (h as f64 - 1.0) * 0.5e-6, "m", w as u64 * item as u64),
    ];
    if nz > 1 {
        dims.push((3, nz as u64, 0.0, -(nz as f64 - 1.0) * 1.5e-6, "m", plane * nc as u64));
    }
    if nt > 1 {
        dims.push((4, nt as u64, 0.0, (nt as f64 - 1.0) * 2.5, "s", plane * (nc * nz) as u64));
    }
    let luts = ["Green", "Red", "Blue", "Gray"];
    let chans = (0..nc).map(|c| (bits, luts[c % 4], c as u64 * plane, 0)).collect();
    let mut data = vec![];
    for t in 0..nt {
        for z in 0..nz {
            for c in 0..nc {
                for y in 0..h {
                    for x in 0..w {
                        let v = pattern(c, z, t, x, y);
                        if item == 2 {
                            data.extend_from_slice(&v.to_le_bytes());
                        } else {
                            data.push(v as u8);
                        }
                    }
                }
            }
        }
    }
    Img { name: name.into(), dims, chans, data, attachments: String::new(), chan_props: vec![] }
}

fn dim_xml(d: &Dim) -> String {
    format!(
        "<DimensionDescription DimID=\"{}\" NumberOfElements=\"{}\" Origin=\"{:e}\" Length=\"{:e}\" Unit=\"{}\" BitInc=\"0\" BytesInc=\"{}\"></DimensionDescription>",
        d.0, d.1, d.2, d.3, d.4, d.5
    )
}

fn chan_xml(c: &Chan, props: &[(&str, &str)]) -> String {
    let (bits, lut, inc, tag) = c;
    let props: String = props
        .iter()
        .map(|(k, v)| format!("<ChannelProperty><Key>{k}</Key><Value>{v}</Value></ChannelProperty>"))
        .collect();
    format!(
        "<ChannelDescription DataType=\"0\" ChannelTag=\"{tag}\" Resolution=\"{bits}\" NameOfMeasuredQuantity=\"\" Min=\"0.0\" Max=\"65535.0\" Unit=\"\" LUTName=\"{lut}\" IsLUTInverted=\"0\" BytesInc=\"{inc}\" BitInc=\"0\">{props}</ChannelDescription>"
    )
}

/// The `<Data>` element of an image.
fn data_xml(i: &Img) -> String {
    let mut x = format!("<Data><Image TextDescription=\"\">{}<ImageDescription><Channels>", i.attachments);
    for (k, c) in i.chans.iter().enumerate() {
        x.push_str(&chan_xml(c, i.chan_props.get(k).map_or(&[][..], |p| &p[..])));
    }
    x.push_str("</Channels><Dimensions>");
    for d in &i.dims {
        x.push_str(&dim_xml(d));
    }
    x.push_str("</Dimensions></ImageDescription><TimeStampList NumberOfTimeStamps=\"1\">1d27ace506a6f90</TimeStampList></Image></Data>");
    x
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

fn xml_block(xml: &str) -> Vec<u8> {
    let chars = xml.encode_utf16().count() as u32;
    let mut b = 0x70u32.to_le_bytes().to_vec();
    b.extend_from_slice(&(chars * 2 + 5).to_le_bytes());
    b.push(0x2A);
    b.extend_from_slice(&chars.to_le_bytes());
    b.extend_from_slice(&utf16(xml));
    b
}

fn element(n: &Node, blocks: &mut Vec<(String, Vec<u8>)>) -> String {
    match n {
        Node::Folder(name, kids) => {
            let mut x = format!(
                "<Element Name=\"{name}\" Visibility=\"1\"><Data/><Memory Size=\"0\" MemoryBlockID=\"MemBlock_f{}\"/><Children>",
                blocks.len()
            );
            for k in kids {
                x.push_str(&element(k, blocks));
            }
            x.push_str("</Children></Element>");
            x
        }
        Node::Image(i) => {
            let id = format!("MemBlock_{}", blocks.len() + 7);
            let x = format!(
                "<Element Name=\"{}\" Visibility=\"1\">{}<Memory Size=\"{}\" MemoryBlockID=\"{id}\"/><Children/></Element>",
                i.name,
                data_xml(i),
                i.data.len()
            );
            blocks.push((id, i.data.clone()));
            x
        }
    }
}

/// Writes a LIF (`version` 1 or 2) whose root element `root` holds `kids`.
pub fn write_lif(path: &Path, version: u32, root: &str, kids: &[Node]) {
    let mut blocks = vec![];
    let mut x = format!(
        "<LMSDataContainerHeader Version=\"{version}\"><Element Name=\"{root}\" Visibility=\"1\"><Data><Experiment Path=\"x.xlef\"/></Data><Memory Size=\"0\" MemoryBlockID=\"MemBlock_0\"/><Children>"
    );
    for k in kids {
        x.push_str(&element(k, &mut blocks));
    }
    x.push_str("</Children></Element></LMSDataContainerHeader>");
    let mut out = xml_block(&x);
    for (id, data) in blocks {
        out.extend_from_slice(&0x70u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.push(0x2A);
        if version >= 2 {
            out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        } else {
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        }
        out.push(0x2A);
        out.extend_from_slice(&(id.encode_utf16().count() as u32).to_le_bytes());
        out.extend_from_slice(&utf16(&id));
        out.extend_from_slice(&data);
    }
    std::fs::write(path, out).unwrap();
}

/// Writes a LOF: the memory block first, then the XML of its one image.
pub fn write_lof(path: &Path, img: &Img) {
    let x = format!(
        "<LMSDataContainerHeader Version=\"2\"><Element Name=\"{}\" Visibility=\"1\">{}<Memory Size=\"{}\" MemoryBlockID=\"MemBlock_0\"/><Children/></Element></LMSDataContainerHeader>",
        img.name,
        data_xml(img),
        img.data.len()
    );
    let mut out = xml_block("LMS_Object_File");
    out.push(0x2A);
    out.extend_from_slice(&2u32.to_le_bytes());
    out.push(0x2A);
    out.extend_from_slice(&0u32.to_le_bytes());
    out.push(0x2A);
    out.extend_from_slice(&(img.data.len() as u64).to_le_bytes());
    out.extend_from_slice(&img.data);
    out.extend_from_slice(&xml_block(&x));
    std::fs::write(path, out).unwrap();
}
