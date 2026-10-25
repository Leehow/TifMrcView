//! Native half of the Zeiss ZEN plugin (`ui/plugins/zen.js`).
//!
//! ZEN, the software of Zeiss light microscopes (LSM / Airyscan, Axio
//! Imager, Celldiscoverer…), saves CZI files whose pixels the core reader
//! (`formats/czi.rs`) decodes. This plugin turns the embedded XML into the
//! acquisition settings shown in the image-info panel: objective, lasers,
//! detectors, pinhole, scan settings, Airyscan processing and so on.

use std::path::Path;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use super::NativePlugin;
use crate::formats::czi::{child, children, metadata_xml_of, text, Node};
use crate::image::MetaGroup;

pub struct Zen;

impl NativePlugin for Zen {
    fn id(&self) -> &'static str {
        "zen"
    }

    fn call(&self, method: &str, args: Value, _body: &[u8]) -> Result<Value> {
        match method {
            // {path} -> null, or {groups, version}
            "read" => {
                let path = Path::new(args["path"].as_str().unwrap_or_default());
                if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("czi")) {
                    return Ok(Value::Null);
                }
                let xml = metadata_xml_of(path)?;
                let Ok(doc) = roxmltree::Document::parse(&xml) else { return Ok(Value::Null) };
                let Some(meta) = child(doc.root_element(), "Metadata") else { return Ok(Value::Null) };
                let version = text(meta, "Information/Application/Version");
                Ok(json!({ "groups": groups(meta), "version": version }))
            }
            _ => bail!("unknown method {method}"),
        }
    }
}

/// Trims float noise such as 0.030000000000000138 to a few decimals.
fn num(s: &str) -> String {
    match s.parse::<f64>() {
        Ok(v) if v.is_finite() => {
            let r = format!("{v:.4}");
            let r = r.trim_end_matches('0').trim_end_matches('.');
            if (r == "0" || r == "-0") && v != 0.0 { format!("{v:.3e}") } else { r.to_string() }
        }
        _ => s.to_string(),
    }
}

struct Group(MetaGroup);

impl Group {
    fn new(name: &str) -> Group {
        Group(MetaGroup { name: name.into(), items: vec![] })
    }
    fn add(&mut self, k: &str, v: Option<String>) {
        if let Some(v) = v.filter(|v| !v.is_empty() && v != "NaN") {
            self.0.items.push([k.into(), v]);
        }
    }
    fn push_to(self, out: &mut Vec<MetaGroup>) {
        if !self.0.items.is_empty() {
            out.push(self.0);
        }
    }
}

/// Groups shown in the info panel, most useful first.
fn groups(meta: Node) -> Vec<MetaGroup> {
    let inf = child(meta, "Information");
    let image = inf.and_then(|i| child(i, "Image"));
    let instrument = inf.and_then(|i| child(i, "Instrument"));
    let chans: Vec<Node> = children(image.and_then(|i| child(i, "Dimensions/Channels")), "Channel").collect();
    let mut ps = [0.0f64; 3];
    for d in children(child(meta, "Scaling/Items"), "Distance") {
        let v: f64 = text(d, "Value").and_then(|v| v.parse().ok()).unwrap_or(0.0) * 1e6;
        match d.attribute("Id") {
            Some("X") => ps[0] = v,
            Some("Y") => ps[1] = v,
            Some("Z") => ps[2] = v,
            _ => {}
        }
    }

    let mut out = vec![];
    let mut g = Group::new("ZEN 图像");
    let app = inf.and_then(|i| child(i, "Application"));
    g.add("软件", app.map(|a| format!("{} {}", text(a, "Name").unwrap_or_default(), text(a, "Version").unwrap_or_default()).trim().to_string()));
    if let Some(i) = image {
        g.add("采集时间", text(i, "AcquisitionDateAndTime"));
        g.add("像素类型", text(i, "PixelType"));
        g.add("位深", text(i, "ComponentBitCount"));
        let dims: Vec<String> = ["X", "Y", "Z", "C", "T", "M", "S", "H"]
            .iter()
            .filter_map(|d| text(i, &format!("Size{d}")).filter(|v| v != "1" || matches!(*d, "X" | "Y")).map(|v| format!("{d}={v}")))
            .collect();
        g.add("尺寸", Some(dims.join("  ")));
        g.add("原始压缩", text(i, "OriginalCompressionMethod"));
    }
    if ps[0] > 0.0 {
        let f = |v: f64| format!("{:.4} µm", v);
        g.add("像素尺寸 X", Some(f(ps[0])));
        g.add("像素尺寸 Y", Some(f(ps[1])));
        if ps[2] > 0.0 {
            g.add("Z 步长", Some(f(ps[2])));
        }
    }
    g.add("用户", inf.and_then(|i| text(i, "Document/UserName")));
    g.push_to(&mut out);

    let mut g = Group::new("ZEN 显微镜");
    if let Some(ins) = instrument {
        for m in children(child(ins, "Microscopes"), "Microscope") {
            g.add("显微镜", m.attribute("Name").map(str::to_string));
            g.add("系统", text(m, "System"));
            g.add("类型", text(m, "Type"));
            g.add("名称", text(m, "UserDefinedName"));
        }
        for o in children(child(ins, "Objectives"), "Objective") {
            g.add("物镜", o.attribute("Name").map(str::to_string).or_else(|| text(o, "Manufacturer/Model")));
            g.add("放大倍数", text(o, "NominalMagnification"));
            g.add("数值孔径 NA", text(o, "LensNA"));
            g.add("浸没介质", text(o, "Immersion"));
            g.add("工作距离 (µm)", text(o, "WorkingDistance"));
        }
    }
    if let Some(i) = image {
        g.add("总放大倍数", text(i, "MicroscopeSettings/EyepieceSettings/TotalMagnification"));
    }
    g.push_to(&mut out);

    for (i, &n) in chans.iter().enumerate() {
        let name = n.attribute("Name").unwrap_or("");
        let mut g = Group::new(&format!("通道 {}：{}", i + 1, name));
        g.add("荧光染料", text(n, "Fluor"));
        g.add("激发波长 (nm)", text(n, "ExcitationWavelength").map(|v| num(&v)));
        g.add("发射波长 (nm)", text(n, "EmissionWavelength").map(|v| num(&v)));
        g.add("检测范围 (nm)", text(n, "DetectionWavelength/Ranges"));
        g.add("采集模式", text(n, "AcquisitionMode"));
        g.add("对比方式", text(n, "ContrastMethod"));
        g.add("照明", text(n, "IlluminationType"));
        let lights = children(child(n, "LightSourcesSettings"), "LightSourceSettings");
        for l in lights {
            let wl = text(l, "Wavelength").map(|v| num(&v));
            let tr = text(l, "Transmission").and_then(|v| v.parse::<f64>().ok()).map(|v| format!("{}%", num(&(v * 100.0).to_string())));
            g.add("激光", match (wl, tr) {
                (Some(w), Some(t)) => Some(format!("{w} nm, {t}")),
                (w, t) => w.map(|w| format!("{w} nm")).or(t),
            });
        }
        g.add("曝光时间 (ms)", text(n, "ExposureTime").and_then(|v| v.parse::<f64>().ok()).map(|v| num(&format!("{}", v / 1e6))));
        g.add("针孔 (AU)", text(n, "PinholeSizeAiry").map(|v| num(&v)));
        g.add("针孔 (µm)", text(n, "PinholeSize").map(|v| num(&v)));
        if let Some(det) = child(n, "DetectorSettings") {
            g.add("探测器", child(det, "Detector").and_then(|d| d.attribute("Id")).map(|s| s.trim_start_matches("Detector:").trim().to_string()));
            g.add("探测器电压 (V)", text(det, "Voltage").map(|v| num(&v)));
            g.add("数字增益", text(det, "DigitalGain").map(|v| num(&v)));
            g.add("偏移", text(det, "Offset").map(|v| num(&v)));
            g.add("分档", text(det, "Binning"));
        }
        if let Some(ls) = child(n, "LaserScanInfo") {
            g.add("像素驻留 (µs)", text(ls, "PixelTime").and_then(|v| v.parse::<f64>().ok()).map(|v| num(&format!("{}", v * 1e6))));
            g.add("帧时间 (s)", text(ls, "FrameTime").map(|v| num(&v)));
            g.add("平均次数", text(ls, "Averaging"));
            g.add("扫描速度", text(ls, "ScanSpeed"));
            g.add("扫描缩放", text(ls, "ZoomX").map(|v| num(&v)));
            g.add("扫描方向", text(ls, "ScanDirection"));
        }
        if let Some(a) = child(n, "AiryscanSettings") {
            g.add("Airyscan 模式", text(a, "Mode"));
        }
        g.push_to(&mut out);
    }

    if let Some(proc_) = inf.and_then(|i| child(i, "Processing")) {
        let mut g = Group::new("ZEN 处理");
        for c in children(child(proc_, "Airyscan/Channels"), "Channel") {
            // "Channel:0" is the first channel.
            let n = c.attribute("Id").and_then(|id| id.strip_prefix("Channel:")?.parse::<usize>().ok()).unwrap_or(0) + 1;
            g.add(&format!("Airyscan 强度（通道 {n}）"), text(c, "SuperResolutionParameter").map(|v| num(&v)));
            g.add(&format!("Airyscan 参数估计（通道 {n}）"), text(c, "ParameterEstimationType"));
            g.add(&format!("Airyscan 反卷积（通道 {n}）"), text(c, "DeconvolutionType"));
        }
        g.add("采样因子", text(proc_, "LSM/SamplingFactor").map(|v| num(&v)));
        g.push_to(&mut out);
    }

    // Everything under Information, flattened, for search.
    if let Some(i) = inf {
        let mut g = Group::new("ZEN 全部参数");
        flatten(i, String::new(), &mut g.0.items);
        g.push_to(&mut out);
    }
    out
}

fn flatten(n: Node, path: String, out: &mut Vec<[String; 2]>) {
    let mut any = false;
    for c in n.children().filter(|c| c.is_element()) {
        any = true;
        let mut p = if path.is_empty() { c.tag_name().name().to_string() } else { format!("{path}/{}", c.tag_name().name()) };
        if let Some(id) = c.attribute("Name").or(c.attribute("Id")) {
            p.push_str(&format!("[{id}]"));
        }
        flatten(c, p, out);
    }
    if !any {
        if let Some(t) = n.text().map(str::trim).filter(|t| !t.is_empty()) {
            out.push([path, t.to_string()]);
        }
    }
}
