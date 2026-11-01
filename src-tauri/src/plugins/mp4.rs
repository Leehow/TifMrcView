//! Minimal MP4 writer for a single H.264 video track.
//!
//! Samples are streamed into `mdat` as they arrive; `moov` goes at the end.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{bail, Result};

pub const TIMESCALE: u32 = 90_000;

pub struct Mp4Writer {
    out: BufWriter<File>,
    width: u16,
    height: u16,
    mdat_start: u64,
    pos: u64,
    sps: Vec<u8>,
    pps: Vec<u8>,
    sizes: Vec<u32>,
    offsets: Vec<u64>,
    durations: Vec<u32>,
    sync: Vec<u32>,
}

/// Splits an Annex B stream into NAL units without start codes.
fn nal_units(mut b: &[u8]) -> Vec<&[u8]> {
    let mut out = vec![];
    let start = |b: &[u8]| -> Option<(usize, usize)> {
        (0..b.len().saturating_sub(2)).find_map(|i| {
            if b[i] == 0 && b[i + 1] == 0 && b[i + 2] == 1 {
                Some(if i > 0 && b[i - 1] == 0 { (i - 1, 4) } else { (i, 3) })
            } else {
                None
            }
        })
    };
    let Some((i, n)) = start(b) else { return out };
    b = &b[i + n..];
    loop {
        match start(b) {
            Some((i, n)) => {
                out.push(&b[..i]);
                b = &b[i + n..];
            }
            None => {
                out.push(b);
                return out;
            }
        }
    }
}

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 8);
    v.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

/// Full box: version 0 plus flags.
fn fbx(kind: &[u8; 4], flags: u32, body: &[u8]) -> Vec<u8> {
    let mut v = flags.to_be_bytes().to_vec();
    v[0] = 0;
    v.extend_from_slice(body);
    bx(kind, &v)
}

struct B(Vec<u8>);
impl B {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
    fn zeros(&mut self, n: usize) -> &mut Self {
        self.0.resize(self.0.len() + n, 0);
        self
    }
    fn matrix(&mut self) -> &mut Self {
        for v in [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
            self.u32(v);
        }
        self
    }
}

impl Mp4Writer {
    pub fn create(path: &Path, width: u32, height: u32) -> Result<Self> {
        let mut out = BufWriter::with_capacity(1 << 20, File::create(path)?);
        let mut ftyp = B(vec![]);
        ftyp.bytes(b"isom").u32(0x200).bytes(b"isomiso2avc1mp41");
        out.write_all(&bx(b"ftyp", &ftyp.0))?;
        let mdat_start = out.stream_position()?;
        // 64-bit mdat header; the size is patched in `finish`.
        out.write_all(&[0, 0, 0, 1])?;
        out.write_all(b"mdat")?;
        out.write_all(&[0; 8])?;
        Ok(Mp4Writer {
            out,
            width: width as u16,
            height: height as u16,
            mdat_start,
            pos: mdat_start + 16,
            sps: vec![],
            pps: vec![],
            sizes: vec![],
            offsets: vec![],
            durations: vec![],
            sync: vec![],
        })
    }

    /// Appends one encoded frame (Annex B) lasting `duration` ticks of [`TIMESCALE`].
    pub fn add(&mut self, annexb: &[u8], duration: u32) -> Result<()> {
        let mut size = 0u32;
        let mut key = false;
        let offset = self.pos;
        for nal in nal_units(annexb) {
            if nal.is_empty() {
                continue;
            }
            match nal[0] & 0x1f {
                7 => {
                    if self.sps.is_empty() {
                        self.sps = nal.to_vec();
                    }
                    continue;
                }
                8 => {
                    if self.pps.is_empty() {
                        self.pps = nal.to_vec();
                    }
                    continue;
                }
                9 => continue, // access unit delimiter
                5 => key = true,
                _ => {}
            }
            self.out.write_all(&(nal.len() as u32).to_be_bytes())?;
            self.out.write_all(nal)?;
            size += 4 + nal.len() as u32;
        }
        if size == 0 {
            // Skipped frame: hold the previous one longer.
            if let Some(d) = self.durations.last_mut() {
                *d += duration;
            }
            return Ok(());
        }
        self.pos += size as u64;
        self.sizes.push(size);
        self.offsets.push(offset);
        self.durations.push(duration);
        if key {
            self.sync.push(self.sizes.len() as u32);
        }
        Ok(())
    }

    pub fn frames(&self) -> usize {
        self.sizes.len()
    }

    /// Writes `moov` and returns the file size.
    pub fn finish(mut self) -> Result<u64> {
        if self.sizes.is_empty() || self.sps.len() < 4 || self.pps.is_empty() {
            bail!("no video frames were encoded");
        }
        let duration: u64 = self.durations.iter().map(|&d| d as u64).sum();
        let dur32 = duration.min(u32::MAX as u64) as u32;
        let (w, h) = (self.width as u32, self.height as u32);

        let mvhd = {
            let mut b = B(vec![]);
            b.u32(0).u32(0).u32(TIMESCALE).u32(dur32).u32(0x0001_0000).u16(0x0100).zeros(10);
            b.matrix().zeros(24).u32(2);
            fbx(b"mvhd", 0, &b.0)
        };
        let tkhd = {
            let mut b = B(vec![]);
            b.u32(0).u32(0).u32(1).u32(0).u32(dur32).zeros(8).u16(0).u16(0).u16(0).u16(0);
            b.matrix().u32(w << 16).u32(h << 16);
            fbx(b"tkhd", 3, &b.0)
        };
        let mdhd = {
            let mut b = B(vec![]);
            b.u32(0).u32(0).u32(TIMESCALE).u32(dur32).u16(0x55c4).u16(0); // "und"
            fbx(b"mdhd", 0, &b.0)
        };
        let hdlr = {
            let mut b = B(vec![]);
            b.u32(0).bytes(b"vide").zeros(12).bytes(b"VideoHandler\0");
            fbx(b"hdlr", 0, &b.0)
        };
        let vmhd = fbx(b"vmhd", 1, &[0; 8]);
        let dinf = {
            let url = fbx(b"url ", 1, &[]);
            let mut b = B(vec![]);
            b.u32(1).bytes(&url);
            bx(b"dinf", &fbx(b"dref", 0, &b.0))
        };
        let avcc = {
            let mut b = B(vec![]);
            b.u8(1).u8(self.sps[1]).u8(self.sps[2]).u8(self.sps[3]).u8(0xff).u8(0xe1);
            b.u16(self.sps.len() as u16).bytes(&self.sps);
            b.u8(1).u16(self.pps.len() as u16).bytes(&self.pps);
            bx(b"avcC", &b.0)
        };
        let avc1 = {
            let mut b = B(vec![]);
            b.zeros(6).u16(1).zeros(16).u16(self.width).u16(self.height);
            b.u32(0x0048_0000).u32(0x0048_0000).u32(0).u16(1);
            let mut name = [0u8; 32];
            let label = b"TifMrcView";
            name[0] = label.len() as u8;
            name[1..=label.len()].copy_from_slice(label);
            b.bytes(&name).u16(0x18).u16(0xffff).bytes(&avcc);
            bx(b"avc1", &b.0)
        };
        let stsd = {
            let mut b = B(vec![]);
            b.u32(1).bytes(&avc1);
            fbx(b"stsd", 0, &b.0)
        };
        let stts = {
            let mut runs: Vec<(u32, u32)> = vec![];
            for &d in &self.durations {
                match runs.last_mut() {
                    Some((n, v)) if *v == d => *n += 1,
                    _ => runs.push((1, d)),
                }
            }
            let mut b = B(vec![]);
            b.u32(runs.len() as u32);
            for (n, d) in runs {
                b.u32(n).u32(d);
            }
            fbx(b"stts", 0, &b.0)
        };
        let stss = {
            let mut b = B(vec![]);
            b.u32(self.sync.len() as u32);
            for &s in &self.sync {
                b.u32(s);
            }
            fbx(b"stss", 0, &b.0)
        };
        let stsc = {
            let mut b = B(vec![]);
            b.u32(1).u32(1).u32(1).u32(1);
            fbx(b"stsc", 0, &b.0)
        };
        let stsz = {
            let mut b = B(vec![]);
            b.u32(0).u32(self.sizes.len() as u32);
            for &s in &self.sizes {
                b.u32(s);
            }
            fbx(b"stsz", 0, &b.0)
        };
        let co64 = {
            let mut b = B(vec![]);
            b.u32(self.offsets.len() as u32);
            for &o in &self.offsets {
                b.u64(o);
            }
            fbx(b"co64", 0, &b.0)
        };
        let stbl = bx(b"stbl", &[stsd, stts, stss, stsc, stsz, co64].concat());
        let minf = bx(b"minf", &[vmhd, dinf, stbl].concat());
        let mdia = bx(b"mdia", &[mdhd, hdlr, minf].concat());
        let trak = bx(b"trak", &[tkhd, mdia].concat());
        let moov = bx(b"moov", &[mvhd, trak].concat());

        self.out.write_all(&moov)?;
        let end = self.pos + moov.len() as u64;
        self.out.seek(SeekFrom::Start(self.mdat_start + 8))?;
        self.out.write_all(&(self.pos - self.mdat_start).to_be_bytes())?;
        self.out.flush()?;
        Ok(end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_annexb() {
        let s = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4, 5];
        let n = nal_units(&s);
        assert_eq!(n, vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 4, 5][..]]);
    }
}
