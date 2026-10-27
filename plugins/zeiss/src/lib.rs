//! Zeiss plugin: every Zeiss format in one TifMrcView WebAssembly plugin.
//!
//! - CZI from ZEN ([`czi`]),
//! - LSM from ZEN black / AIM ([`lsm`]),
//! - the acquisition settings SmartSEM writes into its TIFFs ([`smartsem`]).
//!
//! The host calls `tmv_open` for the extensions plugin.json lists under
//! "formats" and `tmv_metadata` for those under "metadata"; the extension
//! of the path picks the reader.

mod czi;
mod lsm;
mod smartsem;

use tifmrcview_plugin_sdk::{export_format, export_metadata, Format, Info, Recipe, Result};

pub enum Zeiss {
    Czi(czi::Czi),
    Lsm(lsm::Lsm),
}

fn is_lsm(path: &str) -> bool {
    path.rsplit('.').next().is_some_and(|e| e.eq_ignore_ascii_case("lsm"))
}

impl Format for Zeiss {
    fn open(path: &str) -> Result<Zeiss> {
        Ok(if is_lsm(path) { Zeiss::Lsm(lsm::Lsm::open(path)?) } else { Zeiss::Czi(czi::Czi::open(path)?) })
    }

    fn open_series(path: &str, series: u32) -> Result<Zeiss> {
        Ok(if is_lsm(path) {
            Zeiss::Lsm(lsm::Lsm::open_series(path, series)?)
        } else {
            Zeiss::Czi(czi::Czi::open_series(path, series)?)
        })
    }

    fn info(&mut self) -> Result<Info> {
        match self {
            Zeiss::Czi(f) => f.info(),
            Zeiss::Lsm(f) => f.info(),
        }
    }

    fn page(&self, page: u32) -> Result<Recipe> {
        match self {
            Zeiss::Czi(f) => f.page(page),
            Zeiss::Lsm(f) => f.page(page),
        }
    }
}

export_format!(Zeiss);
export_metadata!(smartsem::SmartSem);
