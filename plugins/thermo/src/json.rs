//! A small JSON reader for the metadata Velox stores with every frame:
//! it flattens a document into ("A.B.C", value) pairs, with array items as
//! "A.0", "A.1". Values keep their text (numbers as written, strings
//! unescaped); `null` and empty containers are left out.

pub type Flat = Vec<(String, String)>;

struct P<'a> {
    b: &'a [u8],
    i: usize,
    depth: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\r' | b'\n') {
            self.i += 1;
        }
    }

    fn string(&mut self) -> Option<String> {
        if self.b.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' | b'f' => {}
                        b'u' => {
                            let h = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
                            self.i += 4;
                            let mut cp = u32::from_str_radix(h, 16).ok()?;
                            // Surrogate pair.
                            if (0xd800..0xdc00).contains(&cp)
                                && self.b.get(self.i..self.i + 2) == Some(b"\\u")
                            {
                                let l = std::str::from_utf8(self.b.get(self.i + 2..self.i + 6)?)
                                    .ok()?;
                                if let Ok(lo) = u32::from_str_radix(l, 16) {
                                    if (0xdc00..0xe000).contains(&lo) {
                                        cp = 0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00);
                                        self.i += 6;
                                    }
                                }
                            }
                            let ch = char::from_u32(cp).unwrap_or('\u{fffd}');
                            let mut buf = [0; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        c => out.push(c),
                    }
                }
                c => out.push(c),
            }
        }
        Some(String::from_utf8_lossy(&out).into_owned())
    }

    fn value(&mut self, path: &str, out: &mut Flat) -> Option<()> {
        self.depth += 1;
        if self.depth > 64 {
            return None;
        }
        self.ws();
        let join = |k: &str| {
            if path.is_empty() {
                k.to_string()
            } else {
                format!("{path}.{k}")
            }
        };
        match *self.b.get(self.i)? {
            b'{' => {
                self.i += 1;
                self.ws();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                } else {
                    loop {
                        self.ws();
                        let k = self.string()?;
                        self.ws();
                        if self.b.get(self.i) != Some(&b':') {
                            return None;
                        }
                        self.i += 1;
                        self.value(&join(&k), out)?;
                        self.ws();
                        match self.b.get(self.i)? {
                            b',' => self.i += 1,
                            b'}' => {
                                self.i += 1;
                                break;
                            }
                            _ => return None,
                        }
                    }
                }
            }
            b'[' => {
                self.i += 1;
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                } else {
                    let mut n = 0;
                    loop {
                        self.value(&join(&n.to_string()), out)?;
                        n += 1;
                        self.ws();
                        match self.b.get(self.i)? {
                            b',' => self.i += 1,
                            b']' => {
                                self.i += 1;
                                break;
                            }
                            _ => return None,
                        }
                    }
                }
            }
            b'"' => {
                let s = self.string()?;
                out.push((path.to_string(), s));
            }
            _ => {
                let start = self.i;
                while self.i < self.b.len()
                    && !matches!(
                        self.b[self.i],
                        b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n'
                    )
                {
                    self.i += 1;
                }
                let v = std::str::from_utf8(&self.b[start..self.i]).ok()?;
                if v.is_empty() {
                    return None;
                }
                if v != "null" {
                    out.push((path.to_string(), v.to_string()));
                }
            }
        }
        self.depth -= 1;
        Some(())
    }
}

/// Flattens a JSON document; `None` when it is not valid JSON.
pub fn flatten(b: &[u8]) -> Option<Flat> {
    let mut p = P { b, i: 0, depth: 0 };
    let mut out = Vec::new();
    p.value("", &mut out)?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens() {
        let f = flatten(
            br#" {"a": {"b": "x\"y", "c": [1, 2.5e-9, {"d": true}]}, "e": null, "u": "\u00b5m"} "#,
        )
        .unwrap();
        let want: Vec<(String, String)> = [
            ("a.b", "x\"y"),
            ("a.c.0", "1"),
            ("a.c.1", "2.5e-9"),
            ("a.c.2.d", "true"),
            ("u", "µm"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert_eq!(f, want);
        assert!(flatten(b"{\"a\": ").is_none());
    }
}
