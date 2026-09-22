//! Fix a VI's linker tables in place, without LabVIEW.
//!
//! What a LabVIEW relink actually writes into a packaged VI, measured on the
//! 66 VIs the IDE saved out of the NovaXC venv (2026-09-17) and reproduced
//! with `Save.To Buffer`:
//!
//! - the `VILB` entry (VI to its owning `.lvlib`/`.lvclass`) gets the owner's
//!   `<vilib>`-token absolute path instead of a relative or empty one;
//! - every other relative path in a VI that lives in a plain folder becomes
//!   token-absolute too (inside an `.llb` relative links are canonical and
//!   LabVIEW leaves them alone — OpenG lvdata was loaded and stayed clean);
//! - a link to a member of another library gets the library qualifier in its
//!   qualified name (`[openg_array.lvlib, X.vi]`) and, for block-diagram and
//!   front-panel link kinds, the owner's path in the trailing `VILSPathRef`;
//! - drive-letter absolutes from the build machine become token paths.
//!
//! Nothing else changes. A VI whose tables already say all that loads without
//! anything pending, which is what stops the "save changes?" dialog that an
//! unrelinked venv otherwise raises on project close.
//!
//! The byte layouts are pylabview's (`LVlinkinfo.py`, `LVblock.py`,
//! `LVclasses.py`), which round-trip these files byte-exactly. Two rules from
//! there matter for editing in place: padding before a qualified name (to 4)
//! and before a path (to 2) is relative to the *section* position, so every
//! object after an edit is re-padded; and files older than LabVIEW 2014 carry
//! a qualified-name header in each link section that 2014 dropped, so a table
//! is always rewritten in the file's own version, never transplanted.

use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// RSRC container: read every section, keep the info table verbatim, repack.
// ---------------------------------------------------------------------------

fn be16(b: &[u8], o: usize) -> Result<u16> {
    ensure!(o + 2 <= b.len(), "truncated at {o}");
    Ok(u16::from_be_bytes([b[o], b[o + 1]]))
}

fn be32(b: &[u8], o: usize) -> Result<u32> {
    ensure!(o + 4 <= b.len(), "truncated at {o}");
    Ok(u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]))
}

pub struct Section {
    pub ident: [u8; 4],
    /// Where this section's `data_offset` field sits inside `Rsrc::info`.
    off_field: usize,
    pub data: Vec<u8>,
}

pub struct Rsrc {
    head: [u8; 32],
    /// The block-info table, after its 32-byte header copy, kept byte for byte.
    info: Vec<u8>,
    /// In file order.
    pub sections: Vec<Section>,
}

impl Rsrc {
    pub fn parse(b: &[u8]) -> Result<Rsrc> {
        ensure!(b.len() >= 32 && &b[..6] == b"RSRC\r\n", "not an RSRC file");
        let info_off = be32(b, 16)? as usize;
        let info_sz = be32(b, 20)? as usize;
        let data_off = be32(b, 24)? as usize;
        ensure!(data_off == 32, "unexpected data offset {data_off}");
        ensure!(info_off + info_sz <= b.len() && info_sz >= 52, "info table out of range");
        ensure!(b[info_off..info_off + 32] == b[..32], "second RSRC header differs from the first");
        let info = b[info_off + 32..info_off + info_sz].to_vec();
        // BlockInfoListHeader: int1, int2, int3, blockinfo_offset, blockinfo_size.
        let base = be32(&info, 12)? as usize;
        ensure!(base >= 32, "blockinfo offset inside the header copy");
        let base = base - 32;
        let nblocks = be32(&info, base)? as usize + 1;
        let mut sections = Vec::new();
        for i in 0..nblocks {
            let hdr = base + 4 + 12 * i;
            let ident: [u8; 4] = info[hdr..hdr + 4].try_into()?;
            let scount = be32(&info, hdr + 4)? as usize + 1;
            let soff = be32(&info, hdr + 8)? as usize;
            for j in 0..scount {
                let st = base + soff + 20 * j;
                let doff = be32(&info, st + 12)? as usize;
                let pos = data_off + doff;
                let size = be32(b, pos)? as usize;
                ensure!(pos + 4 + size <= b.len(), "section {} data out of range", String::from_utf8_lossy(&ident));
                sections.push((doff, Section { ident, off_field: st + 12, data: b[pos + 4..pos + 4 + size].to_vec() }));
            }
        }
        sections.sort_by_key(|(doff, _)| *doff);
        Ok(Rsrc { head: b[..32].try_into()?, info, sections: sections.into_iter().map(|(_, s)| s).collect() })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut info = self.info.clone();
        let mut data: Vec<u8> = Vec::new();
        for s in &self.sections {
            while !data.len().is_multiple_of(4) {
                data.push(0);
            }
            let doff = data.len() as u32;
            info[s.off_field..s.off_field + 4].copy_from_slice(&doff.to_be_bytes());
            data.extend_from_slice(&(s.data.len() as u32).to_be_bytes());
            data.extend_from_slice(&s.data);
        }
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
        let mut head = self.head;
        head[16..20].copy_from_slice(&((32 + data.len()) as u32).to_be_bytes());
        head[20..24].copy_from_slice(&((32 + info.len()) as u32).to_be_bytes());
        head[24..28].copy_from_slice(&32u32.to_be_bytes());
        head[28..32].copy_from_slice(&(data.len() as u32).to_be_bytes());
        let mut out = Vec::with_capacity(64 + data.len() + info.len());
        out.extend_from_slice(&head);
        out.extend_from_slice(&data);
        out.extend_from_slice(&head);
        out.extend_from_slice(&info);
        out
    }

    /// (major, minor) from the `LVSR` block, e.g. (20, 0) for LabVIEW 2020.
    pub fn version(&self) -> Result<(u32, u32)> {
        let s = self.sections.iter().find(|s| &s.ident == b"LVSR").context("no LVSR block")?;
        let v = be32(&s.data, 0)?;
        Ok((((v >> 28) & 0xF) * 10 + ((v >> 24) & 0xF), (v >> 20) & 0xF))
    }
}

// ---------------------------------------------------------------------------
// PTH0 path records.
// ---------------------------------------------------------------------------

/// `"PTH0" | u32 length | u16 type | u16 count | pascal components`. Type 0 is
/// absolute — a drive letter or a `<vilib>`-style token as the first
/// component — and type 1 is relative: a leading empty component means "from
/// this VI's folder", each further empty one goes up a level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pth {
    pub tp: u16,
    pub comps: Vec<Vec<u8>>,
    /// Written with a zero length field: the empty placeholder form.
    zero: bool,
}

impl Pth {
    fn parse(b: &[u8], pos: usize) -> Result<(Pth, usize)> {
        ensure!(b.get(pos..pos + 4) == Some(b"PTH0"), "no PTH0 at {pos}");
        let totlen = be32(b, pos + 4)? as usize;
        let tp = be16(b, pos + 8)?;
        let count = be16(b, pos + 10)? as usize;
        ensure!(count <= 128, "PTH0 with {count} components");
        let mut p = pos + 12;
        let mut comps = Vec::with_capacity(count);
        for _ in 0..count {
            let n = *b.get(p).context("truncated PTH0")? as usize;
            ensure!(p + 1 + n <= b.len(), "truncated PTH0 component");
            comps.push(b[p + 1..p + 1 + n].to_vec());
            p += 1 + n;
        }
        let ctlen = 4 + comps.iter().map(|c| 1 + c.len()).sum::<usize>();
        ensure!(totlen == ctlen || totlen == 0, "PTH0 length {totlen} does not match content {ctlen}");
        Ok((Pth { tp, comps, zero: totlen == 0 }, p))
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(b"PTH0");
        let ctlen = 4 + self.comps.iter().map(|c| 1 + c.len()).sum::<usize>();
        let totlen = if self.zero && self.tp == 0 && self.comps.is_empty() { 0 } else { ctlen };
        out.extend_from_slice(&(totlen as u32).to_be_bytes());
        out.extend_from_slice(&self.tp.to_be_bytes());
        out.extend_from_slice(&(self.comps.len() as u16).to_be_bytes());
        for c in &self.comps {
            out.push(c.len() as u8);
            out.extend_from_slice(c);
        }
    }

    pub fn token(comps: Vec<Vec<u8>>) -> Pth {
        Pth { tp: 0, comps, zero: false }
    }

    pub fn is_token(&self) -> bool {
        self.tp == 0 && self.comps.first().is_some_and(|c| c.first() == Some(&b'<'))
    }

    pub fn is_relative(&self) -> bool {
        self.tp == 1
    }

    pub fn is_drive(&self) -> bool {
        self.tp == 0 && self.comps.first().is_some_and(|c| c.len() == 1 && c[0].is_ascii_alphabetic())
    }
}

// ---------------------------------------------------------------------------
// Link sections: LIvi, LIbd, LIfp, LIds.
// ---------------------------------------------------------------------------

// A link-object ident is four ASCII letters or digits (`IUVI`, `DyOM`,
// `H2CC`, ...). Whether a candidate really starts an object is decided by
// parsing its basic part, not by a list: `TCPI` turned up in OpenGDS and is
// in neither pylabview's table nor the 36-kind inventory of this package set.

/// The basic part every link object starts with: ident, qualified name, path, flag, and where it ends.
type Basic = ([u8; 4], Vec<Vec<u8>>, Pth, u32, usize);
/// A nested parent-class link inside a class-API entry: qualified name, absolute path, one flag byte.
type ParentLink = (Vec<Vec<u8>>, Pth, u8);

/// Kinds whose bytes after the basic (qualname, path, flag) part contain no
/// position-dependent padding, per pylabview: typed link info, offset lists,
/// a trailing `VILSPathRef`, self-padded strings, fixed fields. They can
/// shift freely.
const NO_PADS: &[&[u8; 4]] = &[
    b"VILB", b"VIVI", b"VICC", b"VIPV", b"IUVI", b"TDCC", b"PUPV", b"DSDS", b"BSVR", b"SVVI", b"DSSV",
    b"XNXI", b"VIXN", b"H2CC", b"V2CC", b"NEXF", b"DSEF",
];

/// Class-API link kinds — every ident ending in `PI` (`VIPI`, `FPPI`, `DDPI`,
/// `VRPI`, `DRPI`, `RVPI`, and `TCPI`, which pylabview does not know) plus
/// `DyOM` and `PNOM`: basic part, then a 4-aligned API cache (u64 library
/// version, three bools, a long string). The heap variants follow it with
/// another 4-alignment and an offset list; the VI variant ends there. Which
/// one an object is shows in whether anything follows the cache.
fn is_api_kind(id: &[u8; 4]) -> bool {
    &id[2..] == b"PI" || id == b"DyOM" || id == b"PNOM"
}

/// .NET assembly links: an offset list (heap side) or nothing (VI side), then
/// a 2-alignment, then a trailing `VILSPathRef`. The pad is recomputed.
const DN_KINDS: &[&[u8; 4]] = &[b"DNDA", b"DNVA"];

/// Kinds that carry a trailing `VILSPathRef` (owner path of the target).
const VILS_KINDS: &[&[u8; 4]] = &[b"IUVI", b"TDCC", b"PUPV", b"SVVI"];

#[derive(Clone, Debug)]
pub struct LinkObj {
    pub ident: [u8; 4],
    pub qual: Vec<Vec<u8>>,
    pub path: Pth,
    flag: u32,
    /// Everything after the flag up to the next object, as read.
    tail: Vec<u8>,
    /// Section position the tail was read at, for re-padding API caches.
    tail_pos: usize,
}

pub struct LinkSection {
    list_ident: [u8; 4],
    /// Pre-2014 files: the qualified-name header, kept raw.
    header_extra: Vec<u8>,
    pub objs: Vec<LinkObj>,
}

fn pad_to(pos: usize, n: usize) -> usize {
    (n - pos % n) % n
}

/// Parse `ident | pad4 | qualname | pad2 | PTH0 | flag` at `pos` (the ident).
fn parse_basic(b: &[u8], mut pos: usize, ver: (u32, u32)) -> Result<Basic> {
    let ident: [u8; 4] = b.get(pos..pos + 4).context("truncated ident")?.try_into()?;
    pos += 4;
    pos += pad_to(pos, 4);
    let n = be32(b, pos)? as usize;
    ensure!(n <= 64, "qualified name with {n} parts");
    pos += 4;
    let mut qual = Vec::with_capacity(n);
    for _ in 0..n {
        let l = *b.get(pos).context("truncated qualified name")? as usize;
        ensure!(pos + 1 + l <= b.len(), "truncated qualified name part");
        qual.push(b[pos + 1..pos + 1 + l].to_vec());
        pos += 1 + l;
    }
    pos += pad_to(pos, 2);
    let (path, p) = Pth::parse(b, pos)?;
    pos = p;
    let flag = if ver >= (8, 6) {
        let f = be32(b, pos)?;
        pos += 4;
        f
    } else {
        bail!("LabVIEW {}.{} files are older than this tool handles", ver.0, ver.1)
    };
    Ok((ident, qual, path, flag, pos))
}

fn is_known(id: &[u8]) -> bool {
    id.len() == 4 && id.iter().all(|c| c.is_ascii_alphanumeric())
}

impl LinkSection {
    pub fn parse(b: &[u8], ver: (u32, u32)) -> Result<LinkSection> {
        ensure!(be16(b, 0)? == 1, "link section does not start with a root item");
        let list_ident: [u8; 4] = b.get(2..6).context("truncated")?.try_into()?;
        let mut pos = 6;
        if ver < (14, 0) {
            let l = *b.get(pos).context("truncated header")? as usize;
            pos += 1 + l;
            pos += (l + 1) % 2; // pad to 2, counted from the length byte
            let words = be16(b, pos)? as usize;
            pos += 2 + 2 * words;
        }
        let header_extra = b[6..pos].to_vec();
        let count = be32(b, pos)? as usize;
        pos += 4;
        let mut objs = Vec::with_capacity(count);
        loop {
            let marker = be16(b, pos)?;
            if marker == 3 {
                ensure!(pos + 2 == b.len(), "bytes after the end of the link list");
                break;
            }
            ensure!(marker == 2, "unexpected link list marker {marker} at {pos}");
            pos += 2;
            let (ident, qual, path, flag, after) = parse_basic(b, pos, ver)?;
            // The tail runs to the next object start or the list end. Nothing
            // encodes an object's length, so find the next `2 | known ident`
            // whose basic part parses, or the terminating `3` at the very end.
            let mut q = after;
            let end = loop {
                ensure!(q + 2 <= b.len(), "link list never ends");
                if be16(b, q)? == 3 && q + 2 == b.len() {
                    break q;
                }
                if be16(b, q)? == 2 && q + 6 <= b.len() && is_known(&b[q + 2..q + 6]) && parse_basic(b, q + 2, ver).is_ok() {
                    break q;
                }
                q += 1;
            };
            objs.push(LinkObj { ident, qual, path, flag, tail: b[after..end].to_vec(), tail_pos: after });
            pos = end;
        }
        ensure!(objs.len() == count, "link list announced {count} objects, found {}", objs.len());
        Ok(LinkSection { list_ident, header_extra, objs })
    }

    /// `heap`: this is a block-diagram, front-panel or data-space list, whose
    /// class-API links end in an offset list; a VI list (`LIvi`) has none. The
    /// block ident decides, not the list ident — a control's VI list is `LVCC`.
    pub fn write(&self, ver: (u32, u32), heap: bool) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&self.list_ident);
        out.extend_from_slice(&self.header_extra);
        out.extend_from_slice(&(self.objs.len() as u32).to_be_bytes());
        for o in &self.objs {
            out.extend_from_slice(&2u16.to_be_bytes());
            out.extend_from_slice(&o.ident);
            out.resize(out.len() + pad_to(out.len(), 4), 0);
            out.extend_from_slice(&(o.qual.len() as u32).to_be_bytes());
            for q in &o.qual {
                out.push(q.len() as u8);
                out.extend_from_slice(q);
            }
            out.resize(out.len() + pad_to(out.len(), 2), 0);
            o.path.write(&mut out);
            out.extend_from_slice(&o.flag.to_be_bytes());
            let new_pos = out.len();
            if is_api_kind(&o.ident) {
                reflow_api_tail(o, ver, new_pos, heap, &mut out)?;
            } else if DN_KINDS.iter().any(|k| **k == o.ident) {
                reflow_dn_tail(o, ver, new_pos, &mut out)?;
            } else {
                let shifted = (new_pos as isize - o.tail_pos as isize).rem_euclid(4) != 0;
                if shifted && !NO_PADS.iter().any(|k| **k == o.ident) {
                    bail!(
                        "link kind {} moved by a non-multiple of 4 and its layout is not known",
                        String::from_utf8_lossy(&o.ident)
                    );
                }
                out.extend_from_slice(&o.tail);
            }
        }
        out.extend_from_slice(&3u16.to_be_bytes());
        Ok(out)
    }
}

/// A class-API link tail, decoded far enough to re-pad it. Measured layouts
/// (section-position padding marked `pad`):
///
/// ```text
/// LV13   pad4 cache                                  [pad4 offsets]   (heap kinds)
/// LV16/17 pad4 cache five                            [pad4 offsets]
/// LV20   pad4 cache u32 n { qual pad2 PTH0 u8 }×n five [pad4 offsets]
/// ```
///
/// `cache` is u64 library version, three bools, and a long string; `five`
/// is five fixed bytes (`00 00 00 00 ff` on heap kinds). The `n` parent-class
/// links are what a class with a parent carries from LabVIEW 2020 on. Which
/// pieces are present is decided by which combination consumes the tail
/// exactly, so an unfamiliar variant is refused rather than guessed.
struct ApiTail {
    cache: Vec<u8>,
    parents: Option<Vec<ParentLink>>,
    five: Option<[u8; 5]>,
    offsets: Option<Vec<u8>>,
}

fn parse_api_tail(t: &[u8], tail_pos: usize, ver: (u32, u32), heap: bool) -> Result<ApiTail> {
    let mut p = pad_to(tail_pos, 4);
    ensure!(t.len() >= p && t[..p].iter().all(|&x| x == 0), "API cache pad is not zero");
    let start = p;
    p += 8 + if ver >= (9, 0) { 3 } else { 2 };
    let n = be32(t, p)? as usize;
    p += 4 + n;
    ensure!(p <= t.len(), "API cache overruns the object");
    let cache = t[start..p].to_vec();

    // Everything after the cache, tried as each plausible shape.
    let rest_at = p;
    let try_shape = |with_parents: bool, with_five: bool, with_offsets: bool| -> Result<ApiTail> {
        let mut q = rest_at;
        let parents = if with_parents {
            let n = be32(t, q)? as usize;
            ensure!(n <= 16, "too many parent links");
            q += 4;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                let m = be32(t, q)? as usize;
                ensure!(m <= 64, "qualified name too long");
                q += 4;
                let mut qual = Vec::with_capacity(m);
                for _ in 0..m {
                    let l = *t.get(q).context("truncated")? as usize;
                    qual.push(t.get(q + 1..q + 1 + l).context("truncated")?.to_vec());
                    q += 1 + l;
                }
                // Measured on LabVIEW 2020 OpenGDS: the parent path follows the
                // qualified name directly, at odd positions too — no 2-pad here.
                let (pth, e) = Pth::parse(t, q)?;
                q = e;
                let b = *t.get(q).context("truncated")?;
                q += 1;
                v.push((qual, pth, b));
            }
            Some(v)
        } else {
            None
        };
        let five = if with_five {
            let f: [u8; 5] = t.get(q..q + 5).context("truncated")?.try_into()?;
            q += 5;
            Some(f)
        } else {
            None
        };
        let offsets = if with_offsets {
            let k = pad_to(tail_pos + q, 4);
            ensure!(t.get(q..q + k).is_some_and(|z| z.iter().all(|&x| x == 0)), "offset list pad");
            q += k;
            let n = be32(t, q)? as usize;
            ensure!(q + 4 + 4 * n == t.len(), "offset list does not end the tail");
            Some(t[q..].to_vec())
        } else {
            None
        };
        ensure!(q == t.len() || offsets.is_some(), "bytes left over");
        Ok(ApiTail { cache: cache.clone(), parents, five, offsets })
    };
    // A VI-side list (LIvi) never has offsets; a heap-side one always does.
    // Without that split a 9-byte tail reads both as `n=0 parents + five`
    // and as `five + empty offset list`.
    let shapes: &[(bool, bool, bool)] = if heap {
        &[(false, false, false), (false, false, true), (false, true, true), (true, true, true)]
    } else {
        &[(false, false, false), (false, true, false), (true, true, false)]
    };
    let mut hits = Vec::new();
    for &(a, b, c) in shapes {
        if let Ok(x) = try_shape(a, b, c) {
            hits.push(x);
        }
    }
    ensure!(!hits.is_empty(), "class-API link tail has an unknown shape ({} bytes after the cache)", t.len() - rest_at);
    ensure!(hits.len() == 1, "class-API link tail is ambiguous");
    Ok(hits.pop().unwrap())
}

/// Re-emit a class-API link tail at a new position with its pads recomputed.
fn reflow_api_tail(o: &LinkObj, ver: (u32, u32), new_pos: usize, heap: bool, out: &mut Vec<u8>) -> Result<()> {
    let a = parse_api_tail(&o.tail, o.tail_pos, ver, heap)?;
    emit_api_tail(&a, new_pos, out);
    Ok(())
}

fn emit_api_tail(a: &ApiTail, new_pos: usize, out: &mut Vec<u8>) {
    // Pads are section-position relative; `out` may be the section buffer
    // (then out.len() == position) or a fresh tail buffer (then it is not).
    let start = out.len();
    let pos = |out: &Vec<u8>| new_pos + (out.len() - start);
    out.resize(out.len() + pad_to(new_pos, 4), 0);
    out.extend_from_slice(&a.cache);
    if let Some(parents) = &a.parents {
        out.extend_from_slice(&(parents.len() as u32).to_be_bytes());
        for (qual, pth, b) in parents {
            out.extend_from_slice(&(qual.len() as u32).to_be_bytes());
            for q in qual {
                out.push(q.len() as u8);
                out.extend_from_slice(q);
            }
            pth.write(out);
            out.push(*b);
        }
    }
    if let Some(f) = &a.five {
        out.extend_from_slice(f);
    }
    if let Some(list) = &a.offsets {
        let k = pad_to(pos(out), 4);
        out.resize(out.len() + k, 0);
        out.extend_from_slice(list);
    }
}

/// The nested parent-class links of a class-API entry carry the parent
/// class's path in ABSOLUTE form — the build machine's, as packaged — and
/// LabVIEW rewrites them to where the class is now, still absolute. Do the
/// same, so nothing is left for it to rewrite.
fn fix_parent_paths(o: &mut LinkObj, ver: (u32, u32), heap: bool, tree: &Tree) -> Result<bool> {
    let mut a = parse_api_tail(&o.tail, o.tail_pos, ver, heap)?;
    let Some(parents) = a.parents.as_mut() else { return Ok(false) };
    let mut changed = false;
    for (_, pth, _) in parents.iter_mut() {
        if !pth.is_drive() {
            continue;
        }
        let name = String::from_utf8_lossy(pth.comps.last().map(|c| c.as_slice()).unwrap_or(b"")).into_owned();
        let Some(v) = tree.by_name.get(&lower(&name)) else { continue };
        if v.len() != 1 {
            continue;
        }
        let Some(e) = tree.exists(&v[0]) else { continue };
        let Some(abs) = absolute_comps(&e.physical) else { continue };
        if abs != pth.comps {
            pth.comps = abs;
            changed = true;
        }
    }
    if changed {
        // Re-emitted at the position it was read at, pad included, so the
        // stored tail keeps lining up with `tail_pos` for the final write.
        let mut t = Vec::new();
        emit_api_tail(&a, o.tail_pos, &mut t);
        o.tail = t;
    }
    Ok(changed)
}

/// `C:\Git\x\y.lvclass` as LabVIEW's absolute PTH0 components: `["C", "Git", "x", "y.lvclass"]`.
fn absolute_comps(p: &Path) -> Option<Vec<Vec<u8>>> {
    let s = p.to_string_lossy().replace('/', "\\");
    let (drive, rest) = s.split_once(":\\")?;
    let mut v = vec![drive.as_bytes().to_vec()];
    v.extend(rest.split('\\').filter(|x| !x.is_empty()).map(|x| x.as_bytes().to_vec()));
    Some(v)
}

/// Bytes of the typed-link fields that follow the basic part: type index,
/// VI link ref info, typed flags. Fixed by version, no padding.
fn typed_len(t: &[u8], ver: (u32, u32)) -> Result<usize> {
    let mut p = 0;
    let idx = be16(t, p)?;
    p += if idx & 0x8000 != 0 { 4 } else { 2 };
    p += if ver >= (14, 0) { 1 } else { 4 + 8 + 4 + 4 + 4 };
    if ver >= (12, 0) {
        p += 4;
    }
    Ok(p)
}

/// .NET assembly link tail: [typed fields + offset list] (heap side only),
/// a 2-alignment, then the trailing `VILSPathRef`. Recompute the pad.
fn reflow_dn_tail(o: &LinkObj, ver: (u32, u32), new_pos: usize, out: &mut Vec<u8>) -> Result<()> {
    let t = &o.tail;
    let mut p = 0;
    if &o.ident == b"DNDA" {
        p = typed_len(t, ver)?;
        let n = be32(t, p)? as usize;
        p += 4 + 4 * n;
    }
    let k = pad_to(o.tail_pos + p, 2);
    ensure!(t.len() >= p + k && t[p..p + k].iter().all(|&x| x == 0), "assembly link pad is not zero");
    ensure!(t.get(p + k..p + k + 4) == Some(b"PTH0"), "assembly link has no trailing path");
    out.extend_from_slice(&t[..p]);
    out.resize(out.len() + pad_to(new_pos + p, 2), 0);
    out.extend_from_slice(&t[p + k..]);
    Ok(())
}

/// The trailing `VILSPathRef` of a heap-to-VI link, if the kind has one.
fn vils(o: &LinkObj) -> Option<(usize, Pth, usize)> {
    if !VILS_KINDS.iter().any(|k| **k == o.ident) {
        return None;
    }
    let i = o.tail.windows(4).position(|w| w == b"PTH0")?;
    let (p, end) = Pth::parse(&o.tail, i).ok()?;
    Some((i, p, end))
}

fn set_vils(o: &mut LinkObj, p: &Pth) -> bool {
    let Some((i, old, end)) = vils(o) else { return false };
    if &old == p {
        return false;
    }
    let mut t = o.tail[..i].to_vec();
    p.write(&mut t);
    t.extend_from_slice(&o.tail[end..]);
    o.tail = t;
    true
}

// ---------------------------------------------------------------------------
// The tree: logical paths, tokens, library membership.
// ---------------------------------------------------------------------------

/// LabVIEW's path tokens and the folders they stand for.
const TOKENS: &[(&str, &str)] = &[
    ("<vilib>", "vi.lib"),
    ("<userlib>", "user.lib"),
    ("<instrlib>", "instr.lib"),
    ("<resource>", "resource"),
    ("<menus>", "menus"),
    ("<help>", "help"),
    ("<examples>", "examples"),
    ("<project>", "project"),
    ("<templates>", "templates"),
];

/// A file in the mounted tree, addressed the way LabVIEW does: relative to the
/// installation root ("vi.lib/Foo/Bar.vi"), case-insensitively.
#[derive(Clone, Debug)]
struct Entry {
    logical: String,
    physical: PathBuf,
}

pub struct Tree {
    /// lowercase logical path -> entry
    files: HashMap<String, Entry>,
    /// lowercase logical path of a member -> logical path of its owning library
    owner: HashMap<String, String>,
    /// lowercase basename -> logical paths (for resolving build-machine absolutes)
    by_name: HashMap<String, Vec<String>>,
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

fn basename(logical: &str) -> &str {
    logical.rsplit('/').next().unwrap_or(logical)
}

fn parent(logical: &str) -> &str {
    match logical.rfind('/') {
        Some(i) => &logical[..i],
        None => "",
    }
}

impl Tree {
    /// Index every `<root>/<package>/1/<folder>/...` under a venv store, or a
    /// plain LabVIEW-shaped tree when `roots` are given directly.
    pub fn index(roots: &[PathBuf]) -> Result<Tree> {
        let mut files = HashMap::new();
        let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
        for root in roots {
            for (physical, logical) in walk(root)? {
                by_name.entry(lower(basename(&logical))).or_default().push(logical.clone());
                files.insert(lower(&logical), Entry { logical, physical });
            }
        }
        let mut t = Tree { files, owner: HashMap::new(), by_name };
        t.read_libraries()?;
        t.read_llbs()?;
        Ok(t)
    }

    /// Members of every `.llb`, so links INTO an LLB resolve and get their
    /// qualifier (Caraya calling OpenG). The members are never rewritten —
    /// they stay LLB-relative — but each one names its owning library in its
    /// own `VILB` entry, which is where the ownership comes from: the
    /// `.lvlib` inside an LLB is not an RSRC member we could otherwise find.
    fn read_llbs(&mut self) -> Result<()> {
        let llbs: Vec<Entry> =
            self.files.values().filter(|e| lower(&e.logical).ends_with(".llb")).cloned().collect();
        for llb in llbs {
            let Ok(bytes) = std::fs::read(&llb.physical) else { continue };
            for (name, member) in llb_members(&bytes) {
                let logical = format!("{}/{}", llb.logical, name);
                self.by_name.entry(lower(&name)).or_default().push(logical.clone());
                self.files.insert(lower(&logical), Entry { logical: logical.clone(), physical: llb.physical.clone() });
                let Ok(r) = Rsrc::parse(member) else { continue };
                let Ok(ver) = r.version() else { continue };
                let Some(sec) = r.sections.iter().find(|s| &s.ident == b"LIvi") else { continue };
                let Ok(ls) = LinkSection::parse(&sec.data, ver) else { continue };
                let Some(vilb) = ls.objs.iter().find(|o| &o.ident == b"VILB") else { continue };
                let owner = if vilb.path.is_relative() {
                    resolve_relative(&llb.logical, &vilb.path.comps)
                } else if vilb.path.is_token() {
                    let s: Vec<String> = vilb.path.comps.iter().map(|c| String::from_utf8_lossy(c).into_owned()).collect();
                    token_to_logical(&s.join("/"))
                } else {
                    None
                };
                if let Some(owner) = owner {
                    self.files
                        .entry(lower(&owner))
                        .or_insert_with(|| Entry { logical: owner.clone(), physical: llb.physical.clone() });
                    self.owner.insert(lower(&logical), owner);
                }
            }
        }
        Ok(())
    }

    /// A link target counts as present when the file exists, or when it is a
    /// class's private-data control (`X.lvclass/X.ctl`), which lives inside
    /// the class file and has no file of its own.
    fn resolvable(&self, logical: &str) -> bool {
        if self.exists(logical).is_some() {
            return true;
        }
        let p = parent(logical);
        lower(p).ends_with(".lvclass") && self.exists(p).is_some()
    }

    fn read_libraries(&mut self) -> Result<()> {
        let libs: Vec<Entry> = self
            .files
            .values()
            .filter(|e| {
                let l = lower(&e.logical);
                l.ends_with(".lvlib") || l.ends_with(".lvclass")
            })
            .cloned()
            .collect();
        for lib in libs {
            let text = match std::fs::read(&lib.physical) {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(_) => continue,
            };
            // A relative URL in a library file counts from the library file
            // itself as if it were a folder: `../X.vi` beside `lib.lvlib` is
            // `X.vi` in the same folder. Measured on openg_variant.lvlib inside
            // lvdata.llb and on every class in the NovaXC set.
            let base = lib.logical.clone();
            for url in urls(&text) {
                let url = url.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&").replace('\\', "/");
                let Some(member) = self.resolve_url(&url, &base) else { continue };
                // A library's own name never owns itself; friends are not members.
                if lower(&member) == lower(&lib.logical) {
                    continue;
                }
                self.owner.insert(lower(&member), lib.logical.clone());
            }
        }
        Ok(())
    }

    /// `/<vilib>/a/b.vi` or `../x/y.vi` relative to `dir` -> logical path, if it exists.
    fn resolve_url(&self, url: &str, dir: &str) -> Option<String> {
        let logical = if let Some(rest) = url.strip_prefix('/') {
            token_to_logical(rest)?
        } else {
            join_relative(dir, url)?
        };
        self.files.get(&lower(&logical)).map(|e| e.logical.clone())
    }

    fn exists(&self, logical: &str) -> Option<&Entry> {
        self.files.get(&lower(logical))
    }

    /// Owner chain, innermost first: [class, library, ...].
    fn chain(&self, logical: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = lower(logical);
        while let Some(o) = self.owner.get(&cur) {
            if out.len() > 8 || out.contains(o) {
                break;
            }
            out.push(o.clone());
            cur = lower(o);
        }
        out
    }
}

/// The URLs of a library file's MEMBERS. A `.lvclass` also lists its parent
/// class (`Type="Parent"`) and its friends (`Type="Friended Library"`) as
/// `<Item>`s with URLs; taking those as members made `Cursor.lvclass` the
/// owner of `Panel Registry.lvclass` and produced qualified names LabVIEW
/// could not match — it then searched by name, found several, and crashed.
fn urls(xml: &str) -> Vec<String> {
    const MEMBER_TYPES: &[&str] = &["VI", "LVClass", "Library", "Class Private Data", "Control"];
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<Item ") {
        let Some(end) = rest[i..].find('>') else { break };
        let item = &rest[i..i + end];
        rest = &rest[i + end..];
        let attr = |name: &str| -> Option<&str> {
            let k = format!("{name}=\"");
            let s = &item[item.find(&k)? + k.len()..];
            Some(&s[..s.find('"')?])
        };
        let (Some(ty), Some(url)) = (attr("Type"), attr("URL")) else { continue };
        if MEMBER_TYPES.contains(&ty) {
            out.push(url.to_string());
        }
    }
    out
}

/// `<vilib>/a/b` -> `vi.lib/a/b`
fn token_to_logical(p: &str) -> Option<String> {
    let (tok, rest) = match p.find('/') {
        Some(i) => (&p[..i], &p[i + 1..]),
        None => (p, ""),
    };
    let folder = TOKENS.iter().find(|(t, _)| t.eq_ignore_ascii_case(tok))?.1;
    Some(if rest.is_empty() { folder.to_string() } else { format!("{folder}/{rest}") })
}

/// `vi.lib/a/b` -> [`<vilib>`, `a`, `b`]
fn logical_to_token(logical: &str) -> Option<Vec<Vec<u8>>> {
    let (folder, rest) = match logical.find('/') {
        Some(i) => (&logical[..i], &logical[i + 1..]),
        None => (logical, ""),
    };
    let tok = TOKENS.iter().find(|(_, f)| f.eq_ignore_ascii_case(folder))?.0;
    let mut v = vec![tok.as_bytes().to_vec()];
    v.extend(rest.split('/').filter(|s| !s.is_empty()).map(|s| s.as_bytes().to_vec()));
    Some(v)
}

fn join_relative(dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// A LabVIEW relative PTH0: leading "" = from the VI's folder, each further "" = up.
fn resolve_relative(vi_dir: &str, comps: &[Vec<u8>]) -> Option<String> {
    let mut parts: Vec<String> = vi_dir.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect();
    let mut it = comps.iter().peekable();
    if it.peek().is_some_and(|c| c.is_empty()) {
        it.next();
    }
    for c in it {
        if c.is_empty() {
            parts.pop()?;
        } else {
            parts.push(String::from_utf8_lossy(c).into_owned());
        }
    }
    Some(parts.join("/"))
}

/// The files inside an LLB: (name with extension, bytes). An LLB is an RSRC
/// container (type `LVAR`) with its members laid out verbatim, each a whole
/// RSRC file whose second header copy sits at its own info offset — that
/// copy is what tells a real member start from a stray `RSRC` string. The
/// name comes from the member's `TITL` block.
fn llb_members(data: &[u8]) -> Vec<(String, &[u8])> {
    let mut out = Vec::new();
    let mut i = match data.len() > 6 { true => 1, false => return out };
    while let Some(off) = data[i..].windows(6).position(|w| w == b"RSRC\r\n") {
        let s = i + off;
        let (Ok(info_off), Ok(info_sz), Ok(data_off), Ok(data_sz)) =
            (be32(data, s + 16), be32(data, s + 20), be32(data, s + 24), be32(data, s + 28))
        else {
            break;
        };
        let end = (info_off + info_sz).max(data_off + data_sz) as usize;
        let copy = s + info_off as usize;
        let valid = copy + 32 <= data.len()
            && s + end <= data.len()
            && &data[copy..copy + 6] == b"RSRC\r\n"
            && data[s + 16..s + 32] == data[copy + 16..copy + 32];
        if !valid {
            i = s + 1;
            continue;
        }
        let member = &data[s..s + end];
        let ext = match &member[8..12] {
            b"LVIN" => ".vi",
            b"LVCC" => ".ctl",
            _ => ".bin",
        };
        let name = Rsrc::parse(member)
            .ok()
            .and_then(|r| {
                let t = r.sections.iter().find(|x| &x.ident == b"TITL")?;
                let n = *t.data.first()? as usize;
                Some(String::from_utf8_lossy(t.data.get(1..1 + n)?).into_owned())
            })
            .unwrap_or_else(|| format!("member@{s}"));
        out.push((format!("{name}{ext}"), member));
        i = s + end;
    }
    out
}

/// Every file under `root`, as (physical, logical) where logical starts with
/// the LabVIEW folder (`vi.lib/...`). An `.llb` is listed as one file and not
/// entered here; `Tree::read_llbs` indexes its members for resolution only.
fn walk(root: &Path) -> Result<Vec<(PathBuf, String)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let p = e?.path();
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push((p, rel));
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The fix itself.
// ---------------------------------------------------------------------------

#[derive(Default, Debug, Clone)]
pub struct Changes {
    pub owner: usize,
    pub relative: usize,
    pub absolute: usize,
    pub qualifier: usize,
    pub vils: usize,
}

impl Changes {
    pub fn total(&self) -> usize {
        self.owner + self.relative + self.absolute + self.qualifier + self.vils
    }
}

/// What to do to one VI's link sections. Returns the new file bytes when
/// anything changed.
pub fn fix_vi(bytes: &[u8], vi_logical: &str, tree: &Tree) -> Result<(Option<Vec<u8>>, Changes)> {
    let mut rsrc = Rsrc::parse(bytes)?;
    let ver = rsrc.version()?;
    ensure!(ver >= (8, 6), "LabVIEW {}.{} is older than this tool handles", ver.0, ver.1);
    let vi_dir = parent(vi_logical).to_string();
    let vi_chain = tree.chain(vi_logical);
    let mut changes = Changes::default();
    let mut touched = false;

    for sec in rsrc.sections.iter_mut() {
        if !matches!(&sec.ident, b"LIvi" | b"LIbd" | b"LIfp" | b"LIds") {
            continue;
        }
        let mut ls = LinkSection::parse(&sec.data, ver)
            .with_context(|| format!("parsing {}", String::from_utf8_lossy(&sec.ident)))?;
        let mut sec_changed = false;
        for o in ls.objs.iter_mut() {
            // Where does this link point, in logical terms?
            let target: Option<String> = if o.path.is_token() {
                let s: Vec<String> = o.path.comps.iter().map(|c| String::from_utf8_lossy(c).into_owned()).collect();
                token_to_logical(&s.join("/")).filter(|l| tree.resolvable(l))
            } else if o.path.is_relative() {
                resolve_relative(&vi_dir, &o.path.comps).filter(|l| tree.resolvable(l))
            } else if o.path.is_drive() {
                let name = String::from_utf8_lossy(o.path.comps.last().map(|c| c.as_slice()).unwrap_or(b"")).into_owned();
                match tree.by_name.get(&lower(&name)) {
                    Some(v) if v.len() == 1 => Some(v[0].clone()),
                    _ => None,
                }
            } else {
                None
            };

            if is_api_kind(&o.ident)
                && let Ok(true) = fix_parent_paths(o, ver, &sec.ident != b"LIvi", tree)
            {
                changes.absolute += 1;
                sec_changed = true;
            }

            if &o.ident == b"VILB" {
                // The owner is what the library files say, not what the VI recorded.
                let owner = tree.owner.get(&lower(vi_logical)).cloned().or(target);
                if let Some(owner) = owner
                    && !o.path.is_token()
                    && let Some(tok) = logical_to_token(&owner)
                {
                    o.path = Pth::token(tok);
                    changes.owner += 1;
                    sec_changed = true;
                }
                continue;
            }

            let Some(target) = target else { continue };

            // Relative and build-machine absolute paths become token paths.
            if (o.path.is_relative() || o.path.is_drive())
                && let Some(tok) = logical_to_token(&target)
            {
                if o.path.is_relative() {
                    changes.relative += 1;
                } else {
                    changes.absolute += 1;
                }
                o.path = Pth::token(tok);
                sec_changed = true;
            }

            // Qualifier: the target's owner chain with our own chain's common
            // prefix removed, outermost first, then the existing last element
            // (the file name, or the class-API marker).
            if &o.ident == b"DSDS" || o.qual.is_empty() {
                continue;
            }
            let is_api = is_api_kind(&o.ident);
            let mut names: Vec<String> = tree.chain(&target).into_iter().rev().map(|l| basename(&l).to_string()).collect();
            if is_api {
                names.push(basename(&target).to_string());
            }
            let ours: Vec<String> = vi_chain.iter().rev().map(|l| basename(l).to_string()).collect();
            let common = names.iter().zip(ours.iter()).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count();
            let prefix: Vec<Vec<u8>> = names[common..].iter().map(|s| s.as_bytes().to_vec()).collect();
            let last = o.qual.last().cloned().unwrap_or_default();
            let mut want = prefix.clone();
            want.push(last);
            if want != o.qual {
                // A packaged VI may carry a longer chain than this tree can
                // derive when the outer libraries live outside the tree, so a
                // longer chain is kept — but only while every extra element
                // names a library this tree has never seen. An extra element
                // the tree does know is provably wrong: a friend or parent
                // class taken for the owner, or the target's own class put in
                // front of a self link. LabVIEW cannot match such a name and
                // goes searching, and an earlier linkfix build wrote exactly
                // that (measured 2026-09-18, mgi_lib_monitored_actor: 362
                // chains, crash on project load). Rewriting them makes a
                // re-run heal a tree instead of preserving the damage.
                let keep = o.qual.len() > want.len()
                    && o.qual.ends_with(&want)
                    && o.qual[..o.qual.len() - want.len()]
                        .iter()
                        .all(|n| !tree.by_name.contains_key(&lower(&String::from_utf8_lossy(n))));
                if !keep {
                    o.qual = want;
                    changes.qualifier += 1;
                    sec_changed = true;
                }
            }
            if !prefix.is_empty()
                && !is_api
                && let Some(owner) = tree.owner.get(&lower(&target))
                && let Some(tok) = logical_to_token(owner)
                && set_vils(o, &Pth::token(tok))
            {
                changes.vils += 1;
                sec_changed = true;
            } else if let Some((_, v, _)) = vils(o)
                && v.is_relative()
                && let Some(l) = resolve_relative(&vi_dir, &v.comps).filter(|l| tree.resolvable(l))
                && let Some(tok) = logical_to_token(&l)
                && set_vils(o, &Pth::token(tok))
            {
                // A same-library link whose owner ref was already there, but
                // relative: LabVIEW rewrites that to the token form as well.
                changes.vils += 1;
                sec_changed = true;
            }
        }
        if sec_changed {
            sec.data = ls
                .write(ver, &sec.ident != b"LIvi")
                .with_context(|| format!("writing {}", String::from_utf8_lossy(&sec.ident)))?;
            touched = true;
        }
    }
    if !touched {
        return Ok((None, changes));
    }
    Ok((Some(rsrc.write()), changes))
}

/// Which files a link fix looks at.
pub fn is_vi_like(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("vi" | "vim" | "vit" | "ctl" | "ctt")
    )
}

/// Fix every VI under the given roots. Returns per-file outcomes.
pub struct Outcome {
    pub path: PathBuf,
    pub result: Result<Changes>,
}

pub fn fix_tree(roots: &[PathBuf], dry_run: bool) -> Result<(Tree, Vec<Outcome>)> {
    let tree = Tree::index(roots)?;
    let mut outcomes = Vec::new();
    // Anything addressed through an `.llb` path component stays as it is:
    // LabVIEW keeps LLB-relative links, whether the LLB is a real container
    // or, as lvpm currently unpacks them, a folder of that name (OpenG array
    // was loaded through Caraya and stayed clean either way).
    let mut entries: Vec<&Entry> = tree
        .files
        .values()
        .filter(|e| is_vi_like(&e.physical) && !lower(&e.logical).contains(".llb/"))
        .collect();
    entries.sort_by(|a, b| a.logical.cmp(&b.logical));
    for e in entries {
        let bytes = match std::fs::read(&e.physical) {
            Ok(b) => b,
            Err(err) => {
                outcomes.push(Outcome { path: e.physical.clone(), result: Err(err.into()) });
                continue;
            }
        };
        // Only VIs and controls: a `.vim` that is really an LLB (RSRC type
        // LVAR) or anything else in the tree is not ours.
        if bytes.len() < 32 || &bytes[..4] != b"RSRC" || !matches!(&bytes[8..12], b"LVIN" | b"LVCC") {
            continue;
        }
        let result = fix_vi(&bytes, &e.logical, &tree).and_then(|(new, ch)| {
            if let Some(new) = new
                && !dry_run
            {
                let tmp = e.physical.with_extension("lvpm-tmp");
                std::fs::write(&tmp, &new)?;
                std::fs::rename(&tmp, &e.physical)?;
            }
            Ok(ch)
        });
        outcomes.push(Outcome { path: e.physical.clone(), result });
    }
    Ok((tree, outcomes))
}

/// Group roots: every `<store>/<package>/1` under a venv store.
pub fn venv_roots(store: &Path) -> Result<Vec<PathBuf>> {
    let mut roots = Vec::new();
    for e in std::fs::read_dir(store)? {
        let p = e?.path();
        if p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
            let one = p.join("1");
            if one.is_dir() {
                roots.push(one);
            }
        }
    }
    roots.sort();
    Ok(roots)
}

#[allow(dead_code)]
pub fn summarize(outcomes: &[Outcome]) -> BTreeMap<&'static str, usize> {
    let mut m = BTreeMap::new();
    for o in outcomes {
        let k = match &o.result {
            Ok(c) if c.total() > 0 => "changed",
            Ok(_) => "clean",
            Err(_) => "skipped",
        };
        *m.entry(k).or_insert(0) += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Option<Vec<u8>> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools").join("Relink Packages.vi");
        std::fs::read(p).ok()
    }

    #[test]
    fn container_round_trips_byte_exactly() {
        let Some(b) = sample() else { return };
        let r = Rsrc::parse(&b).unwrap();
        assert_eq!(r.write(), b);
    }

    #[test]
    fn link_sections_round_trip_byte_exactly() {
        let Some(b) = sample() else { return };
        let r = Rsrc::parse(&b).unwrap();
        let ver = r.version().unwrap();
        let mut seen = 0;
        for s in &r.sections {
            if matches!(&s.ident, b"LIvi" | b"LIbd" | b"LIfp" | b"LIds") {
                let ls = LinkSection::parse(&s.data, ver).unwrap();
                assert_eq!(ls.write(ver, &s.ident != b"LIvi").unwrap(), s.data, "{}", String::from_utf8_lossy(&s.ident));
                seen += 1;
            }
        }
        assert!(seen > 0);
    }

    #[test]
    fn relative_paths_resolve_like_labview() {
        let up = vec![b"".to_vec(), b"".to_vec(), b"JKI JSON Serialization.lvlib".to_vec()];
        assert_eq!(
            resolve_relative("vi.lib/addons/_JKI.lib/Serialization/JSON/Library", &up).unwrap(),
            "vi.lib/addons/_JKI.lib/Serialization/JSON/JKI JSON Serialization.lvlib"
        );
        let same = vec![b"".to_vec(), b"UTF-8 String.lvclass".to_vec()];
        assert_eq!(resolve_relative("vi.lib/a/b", &same).unwrap(), "vi.lib/a/b/UTF-8 String.lvclass");
    }

    #[test]
    fn tokens_map_both_ways() {
        assert_eq!(token_to_logical("<vilib>/x/y.vi").unwrap(), "vi.lib/x/y.vi");
        let t = logical_to_token("user.lib/_OpenG.lib/a.lvlib").unwrap();
        assert_eq!(t[0], b"<userlib>");
        assert_eq!(t.len(), 3);
    }
}
