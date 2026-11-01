//! Standard or Pro build. Pro is the same app plus the advanced tools, with
//! its own name and identifiers so both can be installed side by side.
//! Build Pro with `--features pro` and `--config tauri.pro.conf.json`.

pub const PRO: bool = cfg!(feature = "pro");
pub const NAME: &str = if PRO { "TifMrcView Pro" } else { "TifMrcView" };
/// Bundle identifier, also the name of the app's data folder.
pub const APP_ID: &str = if PRO { "com.leehow.tifmrcview.pro" } else { "com.leehow.tifmrcview" };
/// Executable name, and the prefix of the files it installs on Linux.
pub const BIN: &str = if PRO { "tifmrcview-pro" } else { "tifmrcview" };
/// Windows Explorer thumbnail provider (tmv-thumbnail-win and the installer hooks).
pub const THUMB_CLSID: u128 = if PRO {
    0x330c748e_26ed_458a_ad27_9b0ce317cde1
} else {
    0x926e1d8f_eb25_48d9_a251_9d27bf165d8a
};
pub const THUMB_CLSID_STR: &str = if PRO {
    "{330c748e-26ed-458a-ad27-9b0ce317cde1}"
} else {
    "{926e1d8f-eb25-48d9-a251-9d27bf165d8a}"
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_tauri_config() {
        let base: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let pro: serde_json::Value = serde_json::from_str(include_str!("../tauri.pro.conf.json")).unwrap();
        let config = if PRO { &pro } else { &base };
        assert_eq!(config["identifier"], APP_ID);
        assert_eq!(config["productName"], NAME);
        assert_eq!(config["mainBinaryName"].as_str().unwrap_or("tifmrcview"), BIN);
        assert_eq!(pro["build"]["features"], serde_json::json!(["pro"]));
    }

    #[test]
    fn clsid_string_matches() {
        let hex = THUMB_CLSID_STR.trim_matches(|c| c == '{' || c == '}').replace('-', "");
        assert_eq!(u128::from_str_radix(&hex, 16).unwrap(), THUMB_CLSID);
    }
}
