//! Development probe: CTF and picking on a folder, with preview PNGs.
use tifmrcview_lib::spa::{pick, project, render};
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let mut p = project::Project::open(&a[1])?;
    p.diameter_a = a[2].parse()?;
    p.optics.pixel_a = a[3].parse()?;
    p.save()?;
    let px = p.optics.pixel_a;
    let cp = tifmrcview_lib::ctf::Params { voltage_kv: 300.0, cs_mm: 2.7, amp_contrast: 0.1, pixel_a: px, res_low_a: 30.0, res_high_a: (px * 2.5).max(4.0), df_min_a: 3000.0, df_max_a: 50000.0, max_astig_a: 2000.0, phase: false, backend: None };
    project::ctf(&mut p, &cp, 512, &|_, _, _| {})?;
    let thr: f64 = a.get(4).map(|s| s.parse().unwrap()).unwrap_or(3.0);
    for (i, m) in p.mics.iter().enumerate() {
        let picks = project::pick_one(&p, i, &pick::PickParams { diameter_a: p.diameter_a, threshold: thr, ..Default::default() })?;
        let (img, w, h) = project::read_mic(&m.path)?;
        let (png, _) = render::mic_preview(&img, w, h, &picks, p.diameter_a / 2.0 / px, 1000)?;
        std::fs::write(format!("{}/pick_{i}.png", p.dir), png)?;
        println!("{} {:?} picks {}", m.path, m.ctf.as_ref().map(|c| (c.df1, c.df2, c.fit_res, c.score)), picks.len());
    }
    Ok(())
}
