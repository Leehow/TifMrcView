//! Thermo Fisher plugin: the electron-microscopy formats of Thermo Fisher
//! Scientific (formerly FEI) in one TifMrcView WebAssembly plugin.
//!
//! - TIA series files, `.ser` ([`ser`]),
//! - Velox `.emd` (HDF5, [`emd`]),
//! - EER electron-event movies of Falcon cameras, `.eer` ([`eer`]),
//! - the settings xT / Helios / Quanta / Scios write into their TIFFs
//!   ([`feitiff`]) and the FEI extended header of MRC files written by EPU,
//!   Tomography and Velox ([`mrc`]), added to files the core opens,
//! - Auto Slice & View projects (`ProjectData.dat`), whose slice images the
//!   app shows as one Z stack ([`asv`]).
//!
//! The extension of the path picks the reader.

mod asv;
mod eer;
mod emd;
mod feitiff;
#[allow(dead_code)] // shared with the Imaris plugin
mod hdf5;
mod json;
mod mrc;
mod ser;

use tifmrcview_plugin_sdk::{
    export_format, export_metadata, export_stack, Format, Info, Meta, Metadata, Recipe, Result,
    Stack, Stacks,
};

pub enum Thermo {
    Ser(ser::Ser),
    Emd(emd::Emd),
    Eer(eer::Eer),
}

fn ext(path: &str) -> String {
    path.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

impl Format for Thermo {
    fn open(path: &str) -> Result<Thermo> {
        Thermo::open_series(path, 0)
    }

    fn open_series(path: &str, series: u32) -> Result<Thermo> {
        Ok(match ext(path).as_str() {
            "emd" => Thermo::Emd(emd::Emd::open(series)?),
            "eer" => Thermo::Eer(eer::Eer::open(series)?),
            _ => Thermo::Ser(ser::Ser::open(path)?),
        })
    }

    fn info(&mut self) -> Result<Info> {
        match self {
            Thermo::Ser(f) => f.info(),
            Thermo::Emd(f) => f.info(),
            Thermo::Eer(f) => f.info(),
        }
    }

    fn page(&self, page: u32) -> Result<Recipe> {
        match self {
            Thermo::Ser(f) => f.page(page),
            Thermo::Emd(f) => f.page(page),
            Thermo::Eer(f) => f.page(page),
        }
    }
}

pub struct ThermoMeta;

impl Metadata for ThermoMeta {
    fn metadata(path: &str) -> Result<Option<Meta>> {
        match ext(path).as_str() {
            "tif" | "tiff" => feitiff::metadata(),
            "mrc" | "mrcs" | "st" | "ali" | "rec" | "map" => mrc::metadata(),
            _ => Ok(None),
        }
    }
}

/// Acquisition projects whose slices are separate images.
pub struct ThermoStacks;

impl Stacks for ThermoStacks {
    fn stack(_path: &str, listing: &[&str], series: u32) -> Result<Stack> {
        asv::stack(listing, series)
    }
}

export_format!(Thermo);
export_metadata!(ThermoMeta);
export_stack!(ThermoStacks);

// ---------------------------------------------------------------- helpers

/// Rounds away the noise of a value stored as f32 or computed in metres.
pub(crate) fn tidy(v: f64) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    format!("{:.9e}", v).parse().unwrap_or(v)
}

pub(crate) fn fmt(v: f64) -> String {
    format!("{}", tidy(v))
}

/// A length in metres as (value, unit) in a unit the viewer knows.
pub(crate) fn length(m: f64) -> (f64, &'static str) {
    let a = m.abs();
    let (f, u) = if a < 1e-10 {
        (1e10, "Å")
    } else if a < 1e-6 {
        (1e9, "nm")
    } else if a < 1e-3 {
        (1e6, "µm")
    } else {
        (1e3, "mm")
    };
    (tidy(m * f), u)
}

/// A pixel size in 1/m (diffraction) as a value in 1/nm.
pub(crate) fn reciprocal(per_m: f64) -> (f64, &'static str) {
    (tidy(per_m * 1e-9), "1/nm")
}

/// A length in metres for display, e.g. "1.2 nm".
pub(crate) fn show_length(m: f64) -> String {
    let (v, u) = length(m);
    format!("{} {u}", fmt(v))
}

/// Seconds since 1970 as "YYYY-MM-DD HH:MM:SS UTC".
pub(crate) fn unix_time(t: i64) -> Option<String> {
    if !(0..=32503680000).contains(&t) {
        return None;
    }
    let days = t.div_euclid(86400);
    let s = t.rem_euclid(86400);
    // Civil date from days since 1970-01-01 (proleptic Gregorian).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    Some(format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        s / 3600,
        s / 60 % 60,
        s % 60
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(length(5.3024148379916015e-9), (5.302414838, "nm"));
        assert_eq!(length(8.17e-12).1, "Å");
        assert_eq!(length(2.5e-5), (25.0, "µm"));
        assert_eq!(
            unix_time(1456073429).as_deref(),
            Some("2016-02-21 16:50:29 UTC")
        );
        assert_eq!(unix_time(0).as_deref(), Some("1970-01-01 00:00:00 UTC"));
        assert_eq!(
            unix_time(951782400).as_deref(),
            Some("2000-02-29 00:00:00 UTC")
        );
    }
}
