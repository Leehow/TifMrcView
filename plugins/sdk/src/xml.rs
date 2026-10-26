//! A small, lenient XML tree for vendor metadata documents (CZI, LIF, OIR…).
//!
//! The plugin runs in an interpreter, where a general-purpose parser spends
//! hundreds of instructions per byte (UTF-8 validation, entity handling and
//! allocations for every node and attribute). This one makes a single pass
//! over the bytes, stores only ranges, and decodes names, attributes and
//! text when they are asked for, which keeps opening a CZI with megabytes
//! of metadata fast. It accepts what vendor software writes: elements, attributes,
//! text, CDATA, comments, processing instructions and a DOCTYPE; anything
//! malformed is skipped rather than rejected.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy)]
struct Elem {
    /// Tag name (without namespace prefix).
    name: (u32, u32),
    /// The attributes part of the start tag.
    attrs: (u32, u32),
    /// The first text run inside the element (may be empty).
    text: (u32, u32),
    /// Whether `text` is CDATA (taken verbatim).
    cdata: bool,
    /// Whether `text` contains `&` (needs unescaping).
    amp: bool,
    /// Value of the `Name` attribute, else of `Id` (escaped), if any.
    label: (u32, u32),
    first_child: u32,
    next_sibling: u32,
    /// Nesting depth (the root is 0).
    depth: u32,
    /// Index after the element's last descendant.
    end: u32,
}

const NONE: u32 = u32::MAX;

pub struct Doc<'a> {
    src: &'a [u8],
    elems: Vec<Elem>,
}

#[derive(Clone, Copy)]
pub struct Node<'d, 'a> {
    doc: &'d Doc<'a>,
    i: u32,
}

fn find(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    let first = needle[0];
    let mut i = from;
    while i + needle.len() <= b.len() {
        i += b[i..].iter().position(|&c| c == first)?;
        if b[i..].starts_with(needle) {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

fn is_name_end(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b'/' | b'>')
}

impl<'a> Doc<'a> {
    pub fn parse(src: &'a [u8]) -> Doc<'a> {
        let mut elems: Vec<Elem> = Vec::with_capacity(src.len() / 64);
        // Open elements, with the last child seen so far.
        let mut stack: Vec<(u32, u32)> = vec![];
        let mut roots_last = NONE;
        let mut i = 0;
        let n = src.len();
        while i < n {
            let Some(k) = src[i..].iter().position(|&c| c == b'<') else { break };
            i += k;
            let rest = &src[i..];
            if rest.starts_with(b"<!--") {
                i = find(src, i + 4, b"-->").map_or(n, |e| e + 3);
                continue;
            }
            if rest.starts_with(b"<![CDATA[") {
                let start = i + 9;
                let end = find(src, start, b"]]>").unwrap_or(n);
                if let Some(&(p, _)) = stack.last() {
                    let e = &mut elems[p as usize];
                    if e.text.0 == e.text.1 {
                        e.text = (start as u32, end as u32);
                        e.cdata = true;
                    }
                }
                i = (end + 3).min(n);
                continue;
            }
            if rest.starts_with(b"<?") || rest.starts_with(b"<!") {
                i = src[i..].iter().position(|&c| c == b'>').map_or(n, |e| i + e + 1);
                continue;
            }
            if rest.starts_with(b"</") {
                let end = src[i..].iter().position(|&c| c == b'>').map_or(n, |e| i + e + 1);
                if let Some((p, _)) = stack.pop() {
                    elems[p as usize].end = elems.len() as u32;
                }
                i = end;
                continue;
            }
            // Start tag: find its end, skipping quoted attribute values and
            // noting the Name / Id attribute on the way.
            let mut j = i + 1;
            let (mut name_attr, mut id_attr) = ((0, 0), (0, 0));
            loop {
                match src[j..].iter().position(|&c| c == b'>' || c == b'"' || c == b'\'') {
                    None => {
                        j = n;
                        break;
                    }
                    Some(k) if src[j + k] == b'>' => {
                        j += k;
                        break;
                    }
                    Some(k) => {
                        let qpos = j + k;
                        let q = src[qpos];
                        let mut kend = qpos;
                        while kend > i + 1 && (is_space(src[kend - 1]) || src[kend - 1] == b'=') {
                            kend -= 1;
                        }
                        let mut kstart = kend;
                        while kstart > i + 1 && !is_space(src[kstart - 1]) {
                            kstart -= 1;
                        }
                        j = qpos + 1;
                        let close = src[j..].iter().position(|&c| c == q).map_or(n, |e| j + e);
                        match &src[kstart..kend] {
                            b"Name" => name_attr = (j as u32, close as u32),
                            b"Id" => id_attr = (j as u32, close as u32),
                            _ => {}
                        }
                        j = close + 1;
                        if j >= n {
                            break;
                        }
                    }
                }
            }
            if j >= n {
                break;
            }
            let self_closing = src[j - 1] == b'/';
            let tag_end = if self_closing { j - 1 } else { j };
            let name_end = (i + 1..tag_end).find(|&p| is_name_end(src[p])).unwrap_or(tag_end);
            let full = &src[i + 1..name_end];
            let local = full.iter().rposition(|&c| c == b':').map_or(i + 1, |c| i + 2 + c);
            let idx = elems.len() as u32;
            let text_start = (j + 1) as u32;
            let mut amp = false;
            let text_end = if self_closing {
                text_start
            } else {
                let mut t = j + 1;
                loop {
                    match src[t..].iter().position(|&c| c == b'<' || c == b'&') {
                        Some(e) if src[t + e] == b'&' => {
                            amp = true;
                            t += e + 1;
                        }
                        Some(e) => break (t + e) as u32,
                        None => break n as u32,
                    }
                }
            };
            elems.push(Elem {
                name: (local as u32, name_end as u32),
                attrs: (name_end as u32, tag_end as u32),
                text: (text_start, text_end),
                cdata: false,
                amp,
                label: if name_attr != (0, 0) { name_attr } else { id_attr },
                first_child: NONE,
                next_sibling: NONE,
                depth: stack.len() as u32,
                end: idx + 1,
            });
            let prev = match stack.last_mut() {
                Some((p, last)) => {
                    let prev = *last;
                    *last = idx;
                    if prev == NONE {
                        elems[*p as usize].first_child = idx;
                    }
                    prev
                }
                None => core::mem::replace(&mut roots_last, idx),
            };
            if prev != NONE {
                elems[prev as usize].next_sibling = idx;
            }
            if !self_closing {
                stack.push((idx, NONE));
            }
            // The text was scanned already; continue at the next tag.
            i = if self_closing { j + 1 } else { text_end as usize };
        }
        // Elements never closed run to the end.
        let total = elems.len() as u32;
        for (p, _) in stack {
            elems[p as usize].end = total;
        }
        Doc { src, elems }
    }

    /// Calls `f(path, text)` for every leaf element below `root`, with paths
    /// like `A/B[label]/C` (see [`Node::label`]). One linear pass over the
    /// elements, which are stored in document order.
    pub fn leaves(&self, root: Node, mut f: impl FnMut(&str, &str)) {
        let r = &self.elems[root.i as usize];
        let base = r.depth + 1;
        let mut path = String::new();
        // Length of `path` before the element at each depth was appended.
        let mut lens: Vec<usize> = vec![];
        for k in root.i + 1..r.end {
            let node = Node { doc: self, i: k };
            let e = &self.elems[k as usize];
            let d = (e.depth - base) as usize;
            if d < lens.len() {
                path.truncate(lens[d]);
                lens.truncate(d);
            }
            lens.push(path.len());
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(&node.name());
            if let Some(l) = node.label() {
                path.push('[');
                path.push_str(&l);
                path.push(']');
            }
            if e.first_child == NONE {
                if let Some(t) = node.text() {
                    f(&path, t.trim());
                }
            }
        }
    }

    pub fn root(&self) -> Option<Node<'_, 'a>> {
        (!self.elems.is_empty()).then_some(Node { doc: self, i: 0 })
    }
}

/// UTF-8 without a copy when valid (the fast path of `from_utf8`, unlike
/// `from_utf8_lossy`, which decodes byte by byte).
fn utf8(b: &[u8]) -> Cow<'_, str> {
    match core::str::from_utf8(b) {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => String::from_utf8_lossy(b),
    }
}

/// Decodes the five named entities and numeric character references.
fn unescape(b: &[u8]) -> Cow<'_, str> {
    let s = utf8(b);
    if !b.contains(&b'&') {
        return s;
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = &s[..];
    while let Some(k) = rest.find('&') {
        out.push_str(&rest[..k]);
        rest = &rest[k..];
        let Some(e) = rest.find(';').filter(|&e| e <= 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..e];
        let c = match ent {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

impl<'d, 'a> Node<'d, 'a> {
    pub fn doc(&self) -> &'d Doc<'a> {
        self.doc
    }

    fn e(&self) -> &Elem {
        &self.doc.elems[self.i as usize]
    }

    fn slice(&self, r: (u32, u32)) -> &'a [u8] {
        &self.doc.src[r.0 as usize..r.1 as usize]
    }

    pub fn name_bytes(&self) -> &'a [u8] {
        self.slice(self.e().name)
    }

    pub fn name(&self) -> Cow<'a, str> {
        utf8(self.name_bytes())
    }

    pub fn has_tag_name(&self, name: &str) -> bool {
        self.name_bytes() == name.as_bytes()
    }

    /// Element children.
    pub fn children(&self) -> Children<'d, 'a> {
        Children { doc: self.doc, next: self.e().first_child }
    }

    /// The (unescaped) value of attribute `name`.
    pub fn attribute(&self, name: &str) -> Option<Cow<'a, str>> {
        let a = self.slice(self.e().attrs);
        let nb = name.as_bytes();
        let mut i = 0;
        while i < a.len() {
            // Skip whitespace to the next attribute name.
            while i < a.len() && a[i].is_ascii_whitespace() {
                i += 1;
            }
            let start = i;
            while i < a.len() && a[i] != b'=' && !a[i].is_ascii_whitespace() {
                i += 1;
            }
            let key = &a[start..i];
            while i < a.len() && (a[i].is_ascii_whitespace() || a[i] == b'=') {
                i += 1;
            }
            let Some(&q) = a.get(i).filter(|&&q| q == b'"' || q == b'\'') else {
                i += 1;
                continue;
            };
            let vstart = i + 1;
            let vend = a[vstart..].iter().position(|&c| c == q).map_or(a.len(), |e| vstart + e);
            if key == nb {
                return Some(unescape(&a[vstart..vend]));
            }
            i = vend + 1;
        }
        None
    }

    /// The element's first text, unescaped (like roxmltree's `Node::text`).
    pub fn text(&self) -> Option<Cow<'a, str>> {
        let e = self.e();
        if e.text.0 == e.text.1 {
            return None;
        }
        let raw = self.slice(e.text);
        Some(if e.cdata || !e.amp { utf8(raw) } else { unescape(raw) })
    }

    /// `attribute("Name")`, else `attribute("Id")`, found while parsing.
    pub fn label(&self) -> Option<Cow<'a, str>> {
        let l = self.e().label;
        (l != (0, 0)).then(|| unescape(self.slice(l)))
    }
}

pub struct Children<'d, 'a> {
    doc: &'d Doc<'a>,
    next: u32,
}

impl<'d, 'a> Iterator for Children<'d, 'a> {
    type Item = Node<'d, 'a>;
    fn next(&mut self) -> Option<Node<'d, 'a>> {
        if self.next == NONE {
            return None;
        }
        let n = Node { doc: self.doc, i: self.next };
        self.next = self.doc.elems[self.next as usize].next_sibling;
        Some(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    const XML: &str = r#"<?xml version="1.0"?>
<!DOCTYPE x>
<ImageDocument><!-- a <comment> -->
  <Metadata>
    <Information Name='info &amp; more'>
      <ns:Image Id="i1" Other="x>y"><Size>  12 </Size><Empty/><Note><![CDATA[a < b]]></Note></ns:Image>
      <Text>&lt;&#65;&#x42;&gt; &bogus; &amp;</Text>
    </Information>
    <Scaling/>
  </Metadata>
</ImageDocument>"#;

    #[test]
    fn tree_attributes_and_text() {
        let doc = Doc::parse(XML.as_bytes());
        let root = doc.root().unwrap();
        assert!(root.has_tag_name("ImageDocument"));
        let meta = root.children().next().unwrap();
        let names: Vec<String> = meta.children().map(|c| c.name().into_owned()).collect();
        assert_eq!(names, ["Information", "Scaling"]);
        let info = meta.children().next().unwrap();
        assert_eq!(info.attribute("Name").as_deref(), Some("info & more"));
        assert_eq!(info.label().as_deref(), Some("info & more"));
        let image = info.children().next().unwrap();
        assert!(image.has_tag_name("Image"), "namespace prefix dropped");
        assert_eq!(image.attribute("Other").as_deref(), Some("x>y"));
        assert_eq!(image.label().as_deref(), Some("i1"));
        assert!(image.attribute("Missing").is_none());
        let kids: Vec<Node> = image.children().collect();
        assert_eq!(kids[0].text().as_deref().map(str::trim), Some("12"));
        assert!(kids[1].text().is_none() && kids[1].children().next().is_none());
        assert_eq!(kids[2].text().as_deref(), Some("a < b"));
        let text = info.children().nth(1).unwrap();
        assert_eq!(text.text().as_deref(), Some("<AB> &bogus; &"));
    }

    #[test]
    fn leaves_are_flattened_in_order() {
        let doc = Doc::parse(XML.as_bytes());
        let info = doc.root().unwrap().children().next().unwrap().children().next().unwrap();
        let mut got = vec![];
        doc.leaves(info, |p, t| got.push(format!("{p}={t}")));
        assert_eq!(got, ["Image[i1]/Size=12", "Image[i1]/Note=a < b", "Text=<AB> &bogus; &"]);
    }

    #[test]
    fn malformed_input_does_not_panic() {
        for s in ["", "<", "<a", "<a b='", "<a><b></a>", "</x>", "<a>&#xFFFFFFFF;</a>", "<!--", "<![CDATA[x", "<a/><b/>"] {
            let doc = Doc::parse(s.as_bytes());
            if let Some(r) = doc.root() {
                let _ = (r.name(), r.text(), r.label(), r.children().count());
                doc.leaves(r, |_, _| {});
            }
        }
    }
}
