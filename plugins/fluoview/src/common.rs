//! Pieces shared by the OIB and OIR readers: turning byte ranges that are
//! scattered over the file into recipe chunks, the FluoView ini format,
//! and number formatting for the info panel.

use tifmrcview_plugin_sdk::{file, Chunk, Codec, Group, Recipe, Result};

pub const DEFAULT_COLORS: [[u8; 3]; 7] =
    [[0, 255, 0], [255, 0, 255], [0, 255, 255], [255, 0, 0], [0, 0, 255], [255, 255, 0], [255, 255, 255]];

/// `len` bytes of a logical byte stream (a plane, a strip) that sit at
/// `file` in the file, starting at logical offset `at`.
#[derive(Clone, Copy, Debug)]
pub struct Piece {
    pub at: u64,
    pub file: u64,
    pub len: u64,
}

/// Adds chunks for `rows` rows of `rb` bytes (`w` pixels each), starting at
/// logical offset 0, drawn from row `y0` of the page. Runs of whole rows
/// inside one piece become file chunks; a row split between pieces (a
/// sector or block boundary) is read and passed inline. Bytes no piece
/// covers stay zero. `pieces` must be sorted by `at` and not overlap.
pub fn raw_rows(pieces: &[Piece], rb: u64, rows: u32, w: u32, y0: u32, flags: u8, r: &mut Recipe) -> Result<()> {
    if rb == 0 || rows == 0 {
        return Ok(());
    }
    let end = rb * rows as u64;
    // Rows that need gluing, in order (each piece adds at most two).
    let mut split: Vec<u32> = vec![];
    for p in pieces {
        let (a, b) = (p.at, (p.at + p.len).min(end));
        if a >= b {
            continue;
        }
        let first = a.div_ceil(rb);
        let last = b / rb;
        if first < last {
            let n = (last - first) as u32;
            r.push(Chunk::file(p.file + (first * rb - a), n as u64 * rb, [0, y0 + first as u32, w, n]).flags(flags));
        }
        // Partial rows at either end of the piece.
        if !a.is_multiple_of(rb) || first > last {
            split.push((a / rb) as u32);
        }
        if !b.is_multiple_of(rb) && last >= first {
            split.push(last as u32);
        }
    }
    split.sort_unstable();
    split.dedup();
    for row in split {
        let (ra, rbnd) = (row as u64 * rb, (row as u64 + 1) * rb);
        let mut buf = vec![0u8; rb as usize];
        let from = pieces.partition_point(|p| p.at + p.len <= ra);
        for p in pieces[from..].iter().take_while(|p| p.at < rbnd) {
            let (a, b) = (p.at.max(ra), (p.at + p.len).min(rbnd));
            if a < b {
                let dst = &mut buf[(a - ra) as usize..(b - ra) as usize];
                file::read_into(p.file + (a - p.at), dst)?;
            }
        }
        r.inline(&buf, Codec::Raw, [0, y0 + row, w, 1]).flags |= flags;
    }
    Ok(())
}

/// Joins raw file chunks that continue each other both in the file and on
/// the page (consecutive strips stored back to back), so a page stored in
/// one piece becomes one chunk (which the host can read in place).
pub fn merge_chunks(r: &mut Recipe) {
    let mut out: Vec<Chunk> = Vec::with_capacity(r.chunks.len());
    for c in r.chunks.drain(..) {
        if let Some(p) = out.last_mut() {
            let joinable = p.source == 0
                && c.source == 0
                && p.codec == Codec::Raw
                && c.codec == Codec::Raw
                && p.flags == c.flags
                && p.stride == 0
                && c.stride == 0
                && p.samples == c.samples
                && p.rect[0] == c.rect[0]
                && p.rect[2] == c.rect[2]
                && p.rect[1] + p.rect[3] == c.rect[1]
                && p.offset + p.length == c.offset;
            if joinable {
                p.length += c.length;
                p.rect[3] += c.rect[3];
                continue;
            }
        }
        out.push(c);
    }
    r.chunks = out;
}

// ---------------------------------------------------------------- ini

/// A FluoView settings file (.oif, .pty, .lut, OibInfo.txt): `[Section]`
/// headers and `Key=Value` lines, in UTF-16 (with BOM) or UTF-8.
pub struct Ini {
    pub sections: Vec<(String, Vec<(String, String)>)>,
}

impl Ini {
    pub fn parse(bytes: &[u8]) -> Ini {
        let text = decode_text(bytes);
        let mut sections: Vec<(String, Vec<(String, String)>)> = vec![];
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                let name = &line[1..line.len() - 1];
                // LUT files end with binary colour data.
                if name == "ColorLUTData" {
                    break;
                }
                sections.push((name.to_string(), vec![]));
            } else if let (Some((k, v)), Some(s)) = (line.split_once('='), sections.last_mut()) {
                let v = v.trim();
                let v = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(v);
                let v = v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')).unwrap_or(v);
                s.1.push((k.trim().to_string(), v.to_string()));
            }
        }
        Ini { sections }
    }

    pub fn section(&self, name: &str) -> Option<&[(String, String)]> {
        self.sections.iter().find(|s| s.0 == name).map(|s| &s.1[..])
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.section(section)?.iter().find(|kv| kv.0 == key).map(|kv| kv.1.as_str())
    }

    /// A non-empty value, as an owned string.
    pub fn s(&self, section: &str, key: &str) -> Option<String> {
        self.get(section, key).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
    }

    pub fn f(&self, section: &str, key: &str) -> Option<f64> {
        self.get(section, key).and_then(fnum)
    }
}

/// UTF-16LE with a byte-order mark, else UTF-8 (lossy).
pub fn decode_text(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xff && b[1] == 0xfe {
        let units = b[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]));
        // Stop at the binary LUT data rather than decoding it.
        let mut s = String::with_capacity(b.len() / 2);
        for c in char::decode_utf16(units) {
            s.push(c.unwrap_or('\u{fffd}'));
            if s.ends_with("[ColorLUTData]") {
                break;
            }
        }
        s
    } else {
        String::from_utf8_lossy(b).into_owned()
    }
}

/// Parses a FluoView number. Files saved on a system with a decimal comma
/// carry values like "0,207.0" (0.207) or "5,3.0" (5.3): the comma is the
/// decimal point and a ".0" was appended.
pub fn fnum(s: &str) -> Option<f64> {
    let s = s.trim().trim_matches('"');
    if let Some((int, rest)) = s.split_once(',') {
        let frac = rest.split('.').next().unwrap_or("");
        if frac.bytes().all(|c| c.is_ascii_digit()) {
            return format!("{int}.{frac}").parse().ok();
        }
    }
    s.parse().ok()
}

/// Trims float noise such as 0.30000000000000004 to a few decimals.
pub fn num(v: f64) -> String {
    if !v.is_finite() {
        return String::new();
    }
    let r = format!("{v:.4}");
    let r = r.trim_end_matches('0').trim_end_matches('.');
    if (r == "0" || r == "-0") && v != 0.0 {
        format!("{v:.3e}")
    } else {
        r.to_string()
    }
}

/// [`num`] for a string value; other text is passed through.
pub fn num_s(s: &str) -> String {
    fnum(s).map(num).unwrap_or_else(|| s.trim().to_string())
}

/// "2026-10-15T10:12:03.98+08:00" -> "2026-10-15 10:12:03 +08:00".
pub fn tidy_date(d: &str) -> String {
    let Some((day, time)) = d.split_once('T') else { return d.to_string() };
    let zone_at = time.find(['+', '-', 'Z']).unwrap_or(time.len());
    let (clock, zone) = time.split_at(zone_at);
    let clock = clock.split('.').next().unwrap_or(clock);
    let zone = if zone == "Z" { "UTC" } else { zone };
    format!("{day} {clock} {zone}").trim_end().to_string()
}

/// "W×H · 2C · 8Z · 5T".
pub fn detail(w: u32, h: u32, c: u32, z: u32, t: u32) -> String {
    let mut s = format!("{w}×{h}");
    for (n, a) in [(c, "C"), (z, "Z"), (t, "T")] {
        if n > 1 {
            s.push_str(&format!(" · {n}{a}"));
        }
    }
    s
}

/// A group that skips empty values.
pub struct G(pub Group);

impl G {
    pub fn new(name: &str) -> G {
        G(Group::new(name))
    }
    pub fn add(&mut self, k: &str, v: Option<String>) {
        self.0.add(k, v.map(|v| v.trim().to_string()));
    }
    pub fn push_to(self, out: &mut Vec<Group>) {
        self.0.push_to(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_with_decimal_commas() {
        assert_eq!(fnum("0,207.0"), Some(0.207));
        assert_eq!(fnum("211,761.0"), Some(211.761));
        assert_eq!(fnum("5,3.0"), Some(5.3));
        assert_eq!(fnum("\"1.600000\""), Some(1.6));
        assert_eq!(fnum("-1530410.0"), Some(-1530410.0));
        assert_eq!(fnum("x"), None);
        assert_eq!(num(0.20700000001), "0.207");
    }

    #[test]
    fn ini_sections_and_quotes() {
        let mut b = vec![0xff, 0xfe];
        for u in "[A]\r\nX=\"q\"\r\nY=1\r\n[ColorLUTData]\r\n".encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        b.extend_from_slice(&[0xff, 0x00, 0x12]);
        let ini = Ini::parse(&b);
        assert_eq!(ini.get("A", "X"), Some("q"));
        assert_eq!(ini.f("A", "Y"), Some(1.0));
        assert_eq!(ini.sections.len(), 1);
    }
}
