//! Olympus FluoView plugin: reads OIB (FV1000 / FV1200) and OIR (FV3000 /
//! FV4000 / cellSens FV) confocal files as a TifMrcView WebAssembly format
//! plugin.
//!
//! References (read, not copied): Christoph Gohlke's oiffile and oirfile
//! (BSD-3-Clause), and Microsoft's public [MS-CFB] specification for the
//! compound file container of OIB.
//!
//! - OIB (`oib.rs`): a compound file (`cfb.rs`) holding one small TIFF per
//!   plane plus ini-style settings files.
//! - OIR (`oir.rs`): a chain of blocks; each plane is a series of raw
//!   strips, metadata is XML.
//!
//! In both cases the plugin only builds an index: each page is handed to
//! the host as file ranges (one per strip, split where a strip crosses a
//! sector boundary), which the host reads natively.

mod cfb;
mod common;
mod oib;
mod oir;

use tifmrcview_plugin_sdk::{bail, export_format, file, Format, Info, Recipe, Result};

pub enum FluoView {
    Oib(oib::Oib),
    Oir(oir::Oir),
}

fn open_any(series: u32) -> Result<FluoView> {
    let head = file::read_at_most(0, 16)?;
    if head.as_slice() == oir::MAGIC {
        return Ok(FluoView::Oir(oir::Oir::open(series)?));
    }
    if cfb::is_cfb(&head) {
        if series > 0 {
            bail!("这个文件只有一个图像（请求第 {} 个）", series + 1);
        }
        return Ok(FluoView::Oib(oib::Oib::open()?));
    }
    bail!("不是 Olympus FluoView 文件（OIB / OIR）")
}

impl Format for FluoView {
    fn open(_path: &str) -> Result<FluoView> {
        open_any(0)
    }

    fn open_series(_path: &str, series: u32) -> Result<FluoView> {
        open_any(series)
    }

    fn info(&mut self) -> Result<Info> {
        Ok(match self {
            FluoView::Oib(f) => f.info(),
            FluoView::Oir(f) => f.info(),
        })
    }

    fn page(&self, page: u32) -> Result<Recipe> {
        match self {
            FluoView::Oib(f) => f.page(page),
            FluoView::Oir(f) => f.page(page),
        }
    }
}

export_format!(FluoView);
