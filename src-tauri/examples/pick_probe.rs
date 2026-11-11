//! Developer tool: the smart selection tool on a real file, without the app.
//! `pick_probe <file> <out.png> <view x0,y0,x1,y1 | all> <min score | -> <path>...`
//! where a path is `x,y` (a click) or `x,y;x,y;...` (a drawn stroke). Each
//! path is selected (red; every reading of it is listed), then the objects
//! like the selected ones are looked for unless the score is `-` (green at or
//! over the score, yellow under it). `TMV_SAM_MODEL=<sam2.1 .pt>` uses that
//! SAM 2.1 checkpoint instead of the model the app would load.
use tifmrcview_lib::image::OpenImage;
use tifmrcview_lib::labels::{ChannelValues, Values};
use tifmrcview_lib::{formats, sam};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let img = OpenImage::new(1, formats::open(a[0].as_ref())?, usize::MAX);
    let info = img.info().clone();
    let (w, h) = (info.width, info.height);
    let (page, sample) = info.locate(0, 0, 0);
    let (lo, hi) = img.stats(page, sample, &img.page(page)?).auto_range();
    let vals = ChannelValues { img: img.clone(), c: 0, t: 0 };
    let view: [f32; 4] = if a[2] == "all" { [0.0, 0.0, w as f32, h as f32] } else { let v: Vec<f32> = a[2].split(',').map(|s| s.parse().unwrap()).collect(); [v[0], v[1], v[2], v[3]] };
    println!("{w}x{h}, model at {:?}", sam::model_path());
    let t = std::time::Instant::now();
    let s = match std::env::var_os("TMV_SAM_MODEL") {
        Some(p) => {
            let (p, backend) = tmv_sam::load_best(p.as_ref(), &tmv_sam::Sam2Config::tiny(), std::time::Duration::from_secs(90))?;
            println!("SAM 2.1 from TMV_SAM_MODEL on {backend}");
            sam::Sam::with_predictor(p)
        }
        None => sam::Sam::default(),
    };
    s.warm()?;
    println!("model ready in {:.1?}", t.elapsed());
    let dn = sam::Denoise { slab: 0, sigma: 0.0 };
    let mut examples: Vec<Vec<usize>> = vec![];
    let mut taken = vec![false; w * h];
    for arg in &a[4..] {
        let path: Vec<[f32; 2]> = arg.split(';').map(|p| { let v: Vec<f32> = p.split(',').map(|s| s.parse().unwrap()).collect(); [v[0], v[1]] }).collect();
        let mut req = sam::PickReq { token: 1, z: 0, c: 0, t: 0, lo, hi, denoise: dn, path, view, choice: None, preview: false };
        let t = std::time::Instant::now();
        let first = s.pick(&req, (w, h, 1), |z| vals.slice(z))?;
        println!("{arg}: {} readings, chose {} (area {}, quality {:.2}) in {:.1?}", first.choices, first.choice, first.mask.iter().filter(|m| **m == 1).count(), first.quality, t.elapsed());
        for c in 0..first.choices {
            req.choice = Some(c);
            let t = std::time::Instant::now();
            let p = s.pick(&req, (w, h, 1), |z| vals.slice(z))?;
            println!("  {c}: rect {:?} area {} quality {:.2} ({:.1?})", p.rect, p.mask.iter().filter(|m| **m == 1).count(), p.quality, t.elapsed());
        }
        let [x0, y0, bw, _] = first.rect;
        let pix: Vec<usize> = first.mask.iter().enumerate().filter(|(_, m)| **m == 1).map(|(i, _)| (y0 + i / bw) * w + x0 + i % bw).collect();
        for &i in &pix { taken[i] = true; }
        examples.push(pix);
    }
    let gray = sam::to_gray(&vals.slice(0)?, lo, hi);
    let mut rgb = image::RgbImage::from_fn(w as u32, h as u32, |x, y| { let g = gray[y as usize * w + x as usize]; image::Rgb([g, g, g]) });
    let mut outline = |rect: [usize; 4], mask: &[bool], col: [u8; 3]| {
        let (bw, bh) = (rect[2], rect[3]);
        for y in 0..bh { for x in 0..bw {
            if !mask[y * bw + x] { continue; }
            let edge = x == 0 || y == 0 || x + 1 == bw || y + 1 == bh || !mask[y * bw + x - 1] || !mask[y * bw + x + 1] || !mask[(y - 1) * bw + x] || !mask[(y + 1) * bw + x];
            if edge { for (dx, dy) in [(0, 0), (1, 0), (0, 1)] { if rect[0] + x + dx < w && rect[1] + y + dy < h { rgb.put_pixel((rect[0] + x + dx) as u32, (rect[1] + y + dy) as u32, image::Rgb(col)); } } }
        } }
    };
    if a[3] != "-" {
        let min: f32 = a[3].parse()?;
        let t = std::time::Instant::now();
        let found = s.similar(&sam::SimilarReq { token: 1, z: 0, c: 0, t: 0, lo, hi, denoise: dn, again: None }, (w, h, 1), |z| vals.slice(z), &examples, &taken, |k, n| eprintln!("  part {k}/{n}"))?;
        println!("{} candidates in {:.1?}; {} at or over {min}", found.len(), t.elapsed(), found.iter().filter(|f| f.score >= min).count());
        // A second round: the weaker half of what passed is called wrong.
        let found = if std::env::var_os("TMV_AGAIN").is_some() {
            let on: Vec<usize> = (0..found.len()).filter(|&i| found[i].score >= min).collect();
            let (keep, drop) = on.split_at(on.len() / 2);
            let t = std::time::Instant::now();
            let again = sam::Again { keep: keep.to_vec(), drop: drop.to_vec() };
            let next = s.similar(&sam::SimilarReq { token: 1, z: 0, c: 0, t: 0, lo, hi, denoise: dn, again: Some(again) }, (w, h, 1), |z| vals.slice(z), &examples, &taken, |_, _| {})?;
            println!("again, {} kept and {} called wrong: {} candidates in {:.1?}; {} at or over {min}", keep.len(), drop.len(), next.len(), t.elapsed(), next.iter().filter(|f| f.score >= min).count());
            next
        } else {
            found
        };
        for f in &found {
            if f.score >= min { outline(f.rect, &f.mask, [60, 230, 90]); } else if f.score >= 0.05 { outline(f.rect, &f.mask, [220, 180, 0]); }
        }
    }
    for e in &examples {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for &i in e { (x0, y0, x1, y1) = (x0.min(i % w), y0.min(i / w), x1.max(i % w), y1.max(i / w)); }
        let bw = x1 - x0 + 1;
        let mut mask = vec![false; bw * (y1 - y0 + 1)];
        for &i in e { mask[(i / w - y0) * bw + i % w - x0] = true; }
        outline([x0, y0, bw, y1 - y0 + 1], &mask, [255, 60, 60]);
    }
    rgb.save(&a[1])?;
    Ok(())
}
