//! A minimal VI Server client.
//!
//! LabVIEW's VI Server speaks an undocumented TCP protocol. We deliberately
//! implement only the smallest useful part of it: five opcodes (hello, resolve
//! a VI path to a reference, call it, release it, goodbye) plus a handful of
//! VI-class method ids — run, save, set and read front-panel values. That is
//! enough to drive a VI by control name, which is how lvpm talks to the VIs
//! that do the real work.
//!
//! That restraint is the whole point. Method ids (`Save:Instrument` is 1002,
//! `Run VI` is 1003, and so on) are internal enum values with no compatibility
//! promise, and they sit in a table that is ordered but not dense, so they
//! cannot even be derived — only observed. Anything built on them needs
//! re-validating against every LabVIEW release; see `docs/vi-server-protocol.md`
//! for how. Everything beyond these primitives belongs in LabVIEW VIs written
//! against the public, documented VI Server API, which NI does keep stable.
//!
//! Wire format, big-endian, confirmed against LabVIEW's own `Server:Logging
//! Enabled` output and loopback captures of both directions:
//!
//! ```text
//! +0  u32 err      +4  u32 opcode      +8  u32 uID      +12 u32 len      then payload
//! ```
//!
//! Replies pair to requests by `uID`. `ReleaseRef` is fire-and-forget — LabVIEW
//! sends nothing back, so we must not block waiting for it.

use crate::target::LvTarget;
use anyhow::{Context, Result, bail, ensure};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

/// Read a `key=value` token out of a target's `LabVIEW.ini`.
fn ini_token(target: &LvTarget, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(target.path.join("LabVIEW.ini")).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key)
            && let Some(v) = rest.trim_start().strip_prefix('=')
        {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// The VI Server port a target listens on, read from its own `LabVIEW.ini`.
///
/// Not a constant: LabVIEW 2025 defaults to 3363 and 2026 to 3364, and both can
/// be installed side by side.
pub fn vi_server_port(target: &LvTarget) -> u16 {
    ini_token(target, "server.tcp.port").and_then(|v| v.parse().ok()).unwrap_or(3363)
}

/// Fail early, and legibly, when a target cannot accept a VI Server client.
pub fn check_vi_server(target: &LvTarget) -> Result<u16> {
    let enabled = ini_token(target, "server.tcp.enabled")
        .map(|v| v.eq_ignore_ascii_case("true"))
        // Absent means LabVIEW's own default, which is on.
        .unwrap_or(true);
    if !enabled {
        bail!(
            "VI Server is disabled for {} (server.tcp.enabled=False in LabVIEW.ini).\n\
             Enable it in Tools >> Options >> VI Server.",
            target.label()
        );
    }
    Ok(vi_server_port(target))
}

// Request opcodes.
const OP_HELLO: u32 = 0;
const OP_GET_VI_REF: u32 = 3;
#[allow(dead_code)] // Call By Reference: needed once we invoke our own batch VIs.
const OP_CALL: u32 = 4;
const OP_VI_DO_METHOD: u32 = 6;
const OP_RELEASE_REF: u32 = 7;
const OP_BYE: u32 = 8;

// Reply opcodes. Not a fixed offset from the request: most are +10, but
// ObjAttrVector is 22->23 and Ping 32->33, so they are separate enum values.
const RET_HELLO: u32 = 10;
const RET_GET_VI_REF: u32 = 13;
#[allow(dead_code)]
const RET_CALL: u32 = 14;
const RET_VI_DO_METHOD: u32 = 16;

/// The handshake payload LabVIEW's own client sends, with our own address
/// spliced in at +28. Captured rather than derived: it embeds a user name
/// ("(Nobody)" when unauthenticated) and the host address, and we have no
/// specification for the remaining fields.
///
/// **LabVIEW validates the address field.** It must be one this machine
/// actually holds; otherwise the server answers err=1379 and closes the
/// connection. Bisected against a live 2026 server: `127.0.0.1` and each of
/// this host's interface addresses handshake fine, while `0.0.0.0` and an
/// address the machine no longer holds are both rejected. So it cannot be a
/// constant — the value originally captured here was a DHCP lease, and the
/// handshake broke the moment the lease moved. Taking it from the connected
/// socket is correct for loopback and stays correct if we ever point this at
/// a LabVIEW on another host.
///
/// Bytes +32..+35 lie beyond the length declared at +4 (`0x1c` = 28) and are
/// ignored: all-zero, all-ones and two separately observed values were each
/// accepted. They are padding, so we send zeros rather than replay a capture's
/// uninitialised bytes.
fn hello_payload(local: Ipv4Addr) -> [u8; 36] {
    let mut p: [u8; 36] = [
        0x26, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x1c, 0x1c, 0x00, 0x00, 0x00, 0x29, 0x00, 0x00,
        0x00, 0x08, 0x00, 0x00, 0x00, b'(', b'N', b'o', b'b', b'o', b'd', b'y', b')', 0, 0, 0, 0,
        0x00, 0x00, 0x00, 0x00,
    ];
    p[28..32].copy_from_slice(&local.octets());
    p
}

/// Options word in a `GetVIRef` request, as sent by LabVIEW's client.
const GET_VI_REF_FLAGS: u32 = 0x8020_0000;

/// The second word of every `VIDoMethod` body. Constant 2 in every observed
/// call regardless of how many parameters follow (Save sends three, `Ctrl
/// Val.Get All` one), so it is a format tag, not an argument count.
const ARGS_FORMAT: u32 = 2;

// ---------------------------------------------------------------------------
// Values and their flattened encoding
// ---------------------------------------------------------------------------

/// LabVIEW type codes as they appear in the low byte of a type descriptor's
/// code word (the high byte carries flags; 0x40 = "has a name"). They match
/// `LvTypeCode` in the Rust-LabVIEW-Interop crate.
const TD_I32: u16 = 0x03;
const TD_DBL: u16 = 0x0a;
const TD_BOOL: u16 = 0x21;
const TD_STRING: u16 = 0x30;
const TD_PATH: u16 = 0x32;
const TD_VARIANT: u16 = 0x53;

/// Flattened sizes of the fixed-size numeric family (codes 0x01..=0x0b:
/// i8..i64, u8..u64, sgl, dbl, ext), used to skip values we do not decode.
fn numeric_size(code: u16) -> Option<usize> {
    match code {
        0x01 | 0x05 => Some(1),
        0x02 | 0x06 => Some(2),
        0x03 | 0x07 | 0x09 => Some(4),
        0x04 | 0x08 | 0x0a => Some(8),
        0x0b => Some(16),
        _ => None,
    }
}

/// A front-panel value lvpm can carry across VI Server.
///
/// The four constructible variants cover what our hook VIs use. `Other` only
/// comes out of decoding: it preserves the raw flattened bytes of a type we
/// recognise well enough to skip but do not interpret.
#[derive(Debug, Clone, PartialEq)]
pub enum LvValue {
    Bool(bool),
    I32(i32),
    Dbl(f64),
    Str(String),
    Other { code: u16, data: Vec<u8> },
}

impl LvValue {
    fn type_code(&self) -> Result<u16> {
        Ok(match self {
            LvValue::Bool(_) => TD_BOOL,
            LvValue::I32(_) => TD_I32,
            LvValue::Dbl(_) => TD_DBL,
            LvValue::Str(_) => TD_STRING,
            LvValue::Other { code, .. } => bail!("cannot encode an undecoded value (type {code:#04x})"),
        })
    }

    /// The value's flattened bytes, without any container padding.
    fn flattened(&self) -> Result<Vec<u8>> {
        Ok(match self {
            LvValue::Bool(b) => vec![*b as u8],
            LvValue::I32(i) => i.to_be_bytes().to_vec(),
            LvValue::Dbl(d) => d.to_be_bytes().to_vec(),
            LvValue::Str(s) => lv_string(s.as_bytes()),
            LvValue::Other { code, .. } => bail!("cannot encode an undecoded value (type {code:#04x})"),
        })
    }
}

impl std::fmt::Display for LvValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LvValue::Bool(b) => write!(f, "{b}"),
            LvValue::I32(i) => write!(f, "{i}"),
            LvValue::Dbl(d) => write!(f, "{d}"),
            LvValue::Str(s) => write!(f, "{s:?}"),
            LvValue::Other { code, data } => {
                write!(f, "<type {code:#04x}:")?;
                for b in data {
                    write!(f, " {b:02x}")?;
                }
                write!(f, ">")
            }
        }
    }
}

/// A LabVIEW string: `u32 length | bytes`, no padding of its own.
fn lv_string(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + bytes.len());
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
    out
}

/// A Pascal string padded to even length, as used for names inside type
/// descriptors and for `PTH0` components... except PTH0 components are NOT
/// padded — only type-descriptor names are.
fn pascal_padded(name: &str) -> Result<Vec<u8>> {
    let bytes = name.as_bytes();
    ensure!(bytes.len() <= 255, "name too long: {name:?}");
    let mut out = Vec::with_capacity(bytes.len() + 2);
    out.push(bytes.len() as u8);
    out.extend_from_slice(bytes);
    if out.len() % 2 == 1 {
        out.push(0);
    }
    Ok(out)
}

/// Build a flattened type descriptor.
///
/// Every length in the observed captures follows one rule set:
///
/// ```text
/// u16 len          total descriptor length, itself included
/// u16 flags|code   0x40 flag = named; code in the low byte
/// [ff ff ff ff]    strings and paths only
/// [00]             fixed-size numerics only (one reserved byte)
/// [pascal name]    padded to even length, when named
/// ```
fn type_desc(code: u16, name: Option<&str>) -> Result<Vec<u8>> {
    let flags: u16 = if name.is_some() { 0x40 } else { 0x00 };
    let mut td = vec![0, 0];
    td.extend_from_slice(&((flags << 8) | code).to_be_bytes());
    if code == TD_STRING || code == TD_PATH {
        td.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);
    }
    if numeric_size(code).is_some() {
        td.push(0);
    }
    if let Some(n) = name {
        td.extend_from_slice(&pascal_padded(n)?);
    }
    let len = td.len() as u16;
    td[0..2].copy_from_slice(&len.to_be_bytes());
    Ok(td)
}

/// The flattening version stamped on variants we produce: LabVIEW 2026
/// release, in LabVIEW's own major/minor/fix/stage encoding. LabVIEW accepts
/// data flattened by older versions, so a fixed stamp is safe.
const FLATTEN_VERSION: u32 = 0x2600_8000;

/// After a type-descriptor table comes a u16 pair selecting the table entry
/// the data conforms to. Single-descriptor containers always carry this value.
const SINGLE_TD: [u8; 4] = [0x00, 0x01, 0x00, 0x00];

/// Flatten a value into a LabVIEW variant:
///
/// ```text
/// u32 version | u32 descriptor count (1) | type descriptor |
/// 00 01 00 00 | flattened value | u32 attribute count (0)
/// ```
///
/// Variants carry no padding of their own; any padding belongs to whatever
/// contains them.
fn variant(v: &LvValue) -> Result<Vec<u8>> {
    let td = type_desc(v.type_code()?, None)?;
    let value = v.flattened()?;
    let mut out = Vec::with_capacity(16 + td.len() + value.len() + 4);
    out.extend_from_slice(&FLATTEN_VERSION.to_be_bytes());
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&td);
    out.extend_from_slice(&SINGLE_TD);
    out.extend_from_slice(&value);
    out.extend_from_slice(&0u32.to_be_bytes());
    Ok(out)
}

/// Build one parameter block of a `VIDoMethod` argument list.
///
/// ```text
/// u32 0x10        block marker
/// u32 blk         everything after this field
/// u32 tdsec       the type-descriptor section: count + descriptor + selector
/// u32 1           descriptor count
/// type descriptor (named — the parameter's name travels here)
/// 00 01 00 00     descriptor selector
/// value           padded to even length
/// ```
fn param(td: Vec<u8>, value: &[u8]) -> Vec<u8> {
    let tdsec = 4 + td.len() + SINGLE_TD.len();
    let padded = value.len() + value.len() % 2;
    let blk = 4 + tdsec + padded;
    let mut out = Vec::with_capacity(12 + blk);
    out.extend_from_slice(&0x10u32.to_be_bytes());
    out.extend_from_slice(&(blk as u32).to_be_bytes());
    out.extend_from_slice(&(tdsec as u32).to_be_bytes());
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&td);
    out.extend_from_slice(&SINGLE_TD);
    out.extend_from_slice(value);
    if value.len() % 2 == 1 {
        out.push(0);
    }
    out
}

/// Frame a method's parameter blocks:
///
/// ```text
/// u32 slots       parameter count + 1, counting the return slot
/// u32 flags       0x01, plus 0x20 when a return-type section follows
/// u32 length      of the return-type section (0 when absent)
/// [return types]
/// parameters...
/// ```
///
/// A method only returns data if the request declares the type it expects —
/// `Ctrl Val.Get` without a return-type section comes back err=0 and empty.
fn method_args(params: &[Vec<u8>], return_types: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(params.len() as u32 + 1).to_be_bytes());
    out.extend_from_slice(&(1u32 | if return_types.is_some() { 0x20 } else { 0 }).to_be_bytes());
    match return_types {
        Some(t) => {
            out.extend_from_slice(&(t.len() as u32).to_be_bytes());
            out.extend_from_slice(t);
        }
        None => out.extend_from_slice(&0u32.to_be_bytes()),
    }
    for p in params {
        out.extend_from_slice(p);
    }
    out
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

/// A VI-class method invoked through `VIDoMethod` (opcode 6).
///
/// Each implementation owns all three version-sensitive facts about one
/// method: its internal id, its parameter names and types, and its reply
/// layout. `Connection::invoke` supplies the transport. Adding a method means
/// adding a struct here and nothing else — and every encoding is regression-
/// tested byte-for-byte against a capture of LabVIEW's own client.
pub trait Method {
    /// What a successful reply decodes to.
    type Output;
    /// The internal method id. An observed enum value, not a published
    /// contract — re-validate per LabVIEW release.
    const ID: u32;
    /// The method's VI Server name, for error messages.
    const NAME: &'static str;
    /// Flatten the arguments (everything after the method id).
    fn encode_args(&self) -> Result<Vec<u8>>;
    /// Decode the reply body. Replies echo the parameter signature with the
    /// values stripped; methods that return data carry it between the
    /// return-type section and that echo.
    fn decode_reply(&self, body: &[u8]) -> Result<Self::Output>;
}

/// `VI: Run VI` — start the VI as if its Run button were pressed.
///
/// With `wait_until_done` the reply only arrives once the VI finishes, so the
/// connection's read timeout must cover the VI's runtime. `auto_dispose_ref`
/// hands the reference's lifetime to the VI: LabVIEW releases it the moment
/// the VI stops, and any later use of the refnum fails with 1026 — so it must
/// be false for the set → run → read-back pattern.
pub struct RunVi {
    pub wait_until_done: bool,
    pub auto_dispose_ref: bool,
}

impl Method for RunVi {
    type Output = ();
    const ID: u32 = 1003; // lvcore_lvprop_vi_run_vi
    const NAME: &'static str = "Run VI";

    fn encode_args(&self) -> Result<Vec<u8>> {
        Ok(method_args(
            &[
                param(type_desc(TD_BOOL, Some("Wait until done"))?, &[self.wait_until_done as u8]),
                param(type_desc(TD_BOOL, Some("Auto Dispose Ref"))?, &[self.auto_dispose_ref as u8]),
            ],
            None,
        ))
    }

    fn decode_reply(&self, _body: &[u8]) -> Result<()> {
        Ok(())
    }
}

/// `VI: Save:Instrument` — an ordinary in-place save (*Save a Copy* and
/// *Without Diagram* both false).
///
/// This is the operation that makes relinking stick: opening a reference is
/// what makes LabVIEW resolve a VI's links, and saving is what persists the
/// resolved paths and recompiled code. Without it LabVIEW redoes the work on
/// every load and never writes anything.
///
/// Prefer checking whether the VI actually changed before calling this.
/// LabVIEW re-saves unconditionally when asked, and observed installs skip a
/// large share of files on that basis — one package had none of its 385 files
/// rewritten.
pub struct SaveInstrument<'a> {
    pub path: &'a Path,
}

impl Method for SaveInstrument<'_> {
    type Output = ();
    const ID: u32 = 1002; // lvcore_lvprop_vi_sve_instrument
    const NAME: &'static str = "Save:Instrument";

    fn encode_args(&self) -> Result<Vec<u8>> {
        Ok(method_args(
            &[
                param(type_desc(TD_PATH, Some("Path to saved file"))?, &encode_pth0(self.path)?),
                param(type_desc(TD_BOOL, Some("Save a Copy"))?, &[0]),
                param(type_desc(TD_BOOL, Some("Without Diagram"))?, &[0]),
            ],
            None,
        ))
    }

    fn decode_reply(&self, _body: &[u8]) -> Result<()> {
        Ok(())
    }
}

/// `VI: Ctrl Val.Set` — write a front-panel control by label.
///
/// The value travels as a variant; LabVIEW answers error 91 if its inner type
/// does not match the control's.
pub struct CtrlValSet<'a> {
    pub control: &'a str,
    pub value: LvValue,
}

impl Method for CtrlValSet<'_> {
    type Output = ();
    const ID: u32 = 1051; // lvcore_lvprop_vi_set_cont_valuevrnt
    const NAME: &'static str = "Ctrl Val.Set";

    fn encode_args(&self) -> Result<Vec<u8>> {
        Ok(method_args(
            &[
                param(type_desc(TD_STRING, Some("Control Name"))?, &lv_string(self.control.as_bytes())),
                param(type_desc(TD_VARIANT, Some("Value"))?, &variant(&self.value)?),
            ],
            None,
        ))
    }

    fn decode_reply(&self, _body: &[u8]) -> Result<()> {
        Ok(())
    }
}

/// Frame a single-descriptor return-type section:
/// `u32 inner length | u32 count (1) | descriptor | selector`.
fn single_return_type(td: Vec<u8>) -> Vec<u8> {
    let inner = 4 + td.len() + SINGLE_TD.len();
    let mut out = Vec::with_capacity(4 + inner);
    out.extend_from_slice(&(inner as u32).to_be_bytes());
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&td);
    out.extend_from_slice(&SINGLE_TD);
    out
}

/// Slice the returned data out of a reply that declared return types: it sits
/// between the echoed return-type section and the parameter echo, padded to
/// even length as a whole.
fn reply_data(body: &[u8]) -> Result<&[u8]> {
    ensure!(body.len() >= 16, "reply too short ({} bytes)", body.len());
    let section = u32::from_be_bytes(body[8..12].try_into().unwrap()) as usize;
    let types = u32::from_be_bytes(body[12..16].try_into().unwrap()) as usize;
    let (start, end) = (12 + 4 + types, 12 + section);
    ensure!(start <= end && end <= body.len(), "malformed method reply");
    Ok(&body[start..end])
}

/// `VI: Ctrl Val.Get` — read one front-panel object by label.
///
/// The value only comes back because the request declares its return type
/// (a variant named after the method's output terminal). LabVIEW's own client
/// omits that section when the output terminal is unwired — and then the
/// reply is an empty echo, err=0 and no data.
pub struct CtrlValGet<'a> {
    pub control: &'a str,
}

impl Method for CtrlValGet<'_> {
    type Output = LvValue;
    const ID: u32 = 1052; // lvcore_lvprop_vi_get_cont_valuevrnt
    const NAME: &'static str = "Ctrl Val.Get";

    fn encode_args(&self) -> Result<Vec<u8>> {
        Ok(method_args(
            &[param(type_desc(TD_STRING, Some("Control Name"))?, &lv_string(self.control.as_bytes()))],
            Some(&single_return_type(type_desc(TD_VARIANT, Some("Get Control Value Variant"))?)),
        ))
    }

    fn decode_reply(&self, body: &[u8]) -> Result<LvValue> {
        Reader::new(reply_data(body)?)
            .variant()
            .with_context(|| format!("decoding control {:?}", self.control))
    }
}

/// The return-type section `Ctrl Val.Get All` must declare: an array of
/// cluster{Name: String, Variant Data: Variant} named "Get All Control Values
/// Variant". Reproduced from a capture because the array and cluster
/// descriptors reference each other through fields we can skip but have not
/// fully decoded; the surrounding framing is generated and understood.
const GET_ALL_RETURN_TYPES: [u8; 98] = [
    0x00, 0x00, 0x00, 0x5e, // section length, this field excluded
    0x00, 0x00, 0x00, 0x04, // four descriptors
    0x00, 0x0e, 0x40, 0x30, 0xff, 0xff, 0xff, 0xff, 0x04, b'N', b'a', b'm', b'e', 0x00, // String "Name"
    0x00, 0x12, 0x40, 0x53, 0x0c, b'V', b'a', b'r', b'i', b'a', b'n', b't', b' ', b'D', b'a',
    b't', b'a', 0x00, // Variant "Variant Data"
    0x00, 0x0a, 0x00, 0x50, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, // array (element: descriptor 2)
    0x00, 0x2c, 0x40, 0x40, 0x00, 0x01, 0xff, 0xff, 0xff, 0xff, 0x00, 0x02, // cluster, 2 fields
    0x1e, b'G', b'e', b't', b' ', b'A', b'l', b'l', b' ', b'C', b'o', b'n', b't', b'r', b'o',
    b'l', b' ', b'V', b'a', b'l', b'u', b'e', b's', b' ', b'V', b'a', b'r', b'i', b'a', b'n',
    b't', 0x00, // its name
    0x00, 0x01, 0x00, 0x03, // one top-level type: descriptor 3, the array
];

/// `VI: Ctrl Val.Get All` — read one whole half of a front panel in a single
/// call, as (label, value) pairs.
///
/// Despite the name it does not return everything. `Controls` is a **selector,
/// not a filter**: TRUE gives the controls, FALSE gives the indicators, and
/// neither gives both. Verified against a live 2026 server on a panel with four
/// of each — TRUE returned exactly the four `… in`, FALSE exactly the four
/// `… out`. Reading a whole panel therefore costs two calls; see
/// [`Connection::ctrl_val_get_panel`].
pub struct CtrlValGetAll {
    /// TRUE selects the controls, FALSE the indicators.
    pub controls: bool,
}

impl Method for CtrlValGetAll {
    type Output = Vec<(String, LvValue)>;
    const ID: u32 = 1053; // lvcore_lvprop_vi_get_all_cont_valuvrnt
    const NAME: &'static str = "Ctrl Val.Get All";

    fn encode_args(&self) -> Result<Vec<u8>> {
        Ok(method_args(
            &[param(type_desc(TD_BOOL, Some("Controls"))?, &[self.controls as u8])],
            Some(&GET_ALL_RETURN_TYPES),
        ))
    }

    fn decode_reply(&self, body: &[u8]) -> Result<Vec<(String, LvValue)>> {
        let mut r = Reader::new(reply_data(body)?);
        let n = r.u32()?;
        let mut out = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let name = r.lv_string()?;
            let value = r.variant().with_context(|| format!("decoding control {name:?}"))?;
            out.push((String::from_utf8_lossy(&name).into_owned(), value));
        }
        Ok(out)
    }
}

/// A bounds-checked cursor over a reply.
struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Reader { b, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(self.pos + n <= self.b.len(), "reply truncated at byte {}", self.pos);
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn lv_string(&mut self) -> Result<Vec<u8>> {
        let len = self.u32()? as usize;
        Ok(self.take(len)?.to_vec())
    }

    /// Decode one variant. Variants inside a reply name their type descriptor
    /// after the control's label; the descriptor's own length field lets us
    /// skip whatever it carries.
    fn variant(&mut self) -> Result<LvValue> {
        let _version = self.u32()?;
        let count = self.u32()?;
        ensure!(count == 1, "variant with {count} type descriptors is unsupported");
        let td_len = self.u16()? as usize;
        let code = self.u16()? & 0x00ff;
        ensure!(td_len >= 4, "malformed type descriptor");
        self.take(td_len - 4)?; // the rest of the descriptor: reserved fields, name
        let selector = self.take(4)?;
        ensure!(selector == SINGLE_TD, "unexpected descriptor selector {selector:02x?}");

        let value = match code {
            TD_BOOL => LvValue::Bool(self.take(1)?[0] != 0),
            TD_I32 => LvValue::I32(i32::from_be_bytes(self.take(4)?.try_into().unwrap())),
            TD_DBL => LvValue::Dbl(f64::from_be_bytes(self.take(8)?.try_into().unwrap())),
            TD_STRING => LvValue::Str(String::from_utf8_lossy(&self.lv_string()?).into_owned()),
            other => match numeric_size(other) {
                // Recognised size: preserve the bytes so the rest of the
                // reply stays parseable.
                Some(n) => LvValue::Other { code: other, data: self.take(n)?.to_vec() },
                // Unknown size means we cannot find the next element.
                None => bail!("control type {other:#04x} is not supported"),
            },
        };
        let attrs = self.u32()?;
        ensure!(attrs == 0, "variant attributes are unsupported ({attrs} present)");
        Ok(value)
    }
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// A message as it appears on the wire.
#[derive(Debug)]
pub struct Message {
    pub err: i32,
    pub opcode: u32,
    pub uid: u32,
    pub body: Vec<u8>,
}

/// An open VI Server connection.
pub struct Connection {
    sock: TcpStream,
    /// Request ids stride by 0x100000; the low 20 bits stay clear.
    next_uid: u32,
}

/// Encode a path as LabVIEW's `PTH0` record.
///
/// The same structure appears inside a VI's `LIvi`/`LIbd` linker blocks, so this
/// encoder serves both the wire protocol and any future offline path rewriting:
/// `"PTH0" | u32 byte length | u32 component count | pascal components`, with an
/// absolute Windows path contributing its drive letter as the first component.
pub fn encode_pth0(path: &Path) -> Result<Vec<u8>> {
    let s = path.to_string_lossy().replace('/', "\\");
    let mut parts: Vec<String> = Vec::new();
    for (i, comp) in s.split('\\').filter(|c| !c.is_empty()).enumerate() {
        // "C:" becomes the component "C".
        if i == 0 && comp.len() == 2 && comp.ends_with(':') {
            parts.push(comp[..1].to_string());
        } else {
            parts.push(comp.to_string());
        }
    }
    if parts.is_empty() {
        bail!("cannot encode empty path");
    }

    let mut components = Vec::new();
    for p in &parts {
        let bytes = p.as_bytes();
        if bytes.len() > 255 {
            bail!("path component too long for PTH0: {p:?}");
        }
        components.push(bytes.len() as u8);
        components.extend_from_slice(bytes);
    }

    let mut out = Vec::with_capacity(components.len() + 16);
    out.extend_from_slice(b"PTH0");
    // Length covers the component count field plus the components themselves.
    out.extend_from_slice(&((components.len() + 4) as u32).to_be_bytes());
    out.extend_from_slice(&(parts.len() as u32).to_be_bytes());
    out.extend_from_slice(&components);
    Ok(out)
}

/// What a VI Server error code means, for the ones we have met.
fn describe_error(code: i32) -> &'static str {
    match code {
        7 => " (file not found)",
        91 => " (variant type mismatch — the value does not match the control's type)",
        1026 => " (VI reference is invalid — released, or auto-disposed by Run VI?)",
        1031 => " (connector pane does not match)",
        1032 => " (VI Server access denied)",
        1379 => " (the address claimed in the handshake is not one this machine holds)",
        _ => "",
    }
}

impl Connection {
    /// Connect and complete the handshake.
    pub fn connect(host: &str, port: u16, timeout: Duration) -> Result<Connection> {
        let addr = format!("{host}:{port}");
        let sock = TcpStream::connect(&addr).with_context(|| format!("connecting to {addr}"))?;
        sock.set_read_timeout(Some(timeout))?;
        sock.set_write_timeout(Some(timeout))?;
        sock.set_nodelay(true)?;

        // Our own address, which the handshake has to claim truthfully — see
        // `hello_payload`. Loopback for anything that is not IPv4.
        let local = match sock.local_addr() {
            Ok(SocketAddr::V4(a)) => *a.ip(),
            _ => Ipv4Addr::LOCALHOST,
        };

        let mut conn = Connection { sock, next_uid: 0x0010_0000 };
        // The hello uses a fixed id, and LabVIEW answers with an id of its own
        // choosing rather than echoing ours, so this one cannot be paired.
        conn.send(OP_HELLO, 0x0000_0006, &hello_payload(local))?;
        let reply = conn.recv()?;
        if reply.opcode != RET_HELLO {
            bail!(
                "unexpected handshake reply: opcode {} (expected {RET_HELLO}). \
                 The VI Server protocol is undocumented and may differ on this \
                 LabVIEW version.",
                reply.opcode
            );
        }
        // A rejected handshake still answers with opcode 10 and then closes the
        // socket, so without this the failure surfaces much later as a connection
        // reset on some unrelated request.
        if reply.err != 0 {
            bail!(
                "VI Server refused the handshake: error {}{}",
                reply.err,
                describe_error(reply.err)
            );
        }
        Ok(conn)
    }

    fn take_uid(&mut self) -> u32 {
        let uid = self.next_uid;
        self.next_uid = self.next_uid.wrapping_add(0x0010_0000);
        uid
    }

    fn send(&mut self, opcode: u32, uid: u32, body: &[u8]) -> Result<()> {
        let mut msg = Vec::with_capacity(16 + body.len());
        msg.extend_from_slice(&0u32.to_be_bytes()); // err, always 0 from a client
        msg.extend_from_slice(&opcode.to_be_bytes());
        msg.extend_from_slice(&uid.to_be_bytes());
        msg.extend_from_slice(&(body.len() as u32).to_be_bytes());
        msg.extend_from_slice(body);
        self.sock.write_all(&msg).context("writing VI Server message")?;
        self.sock.flush()?;
        Ok(())
    }

    fn recv(&mut self) -> Result<Message> {
        let mut head = [0u8; 16];
        self.sock.read_exact(&mut head).context("reading VI Server header")?;
        let err = i32::from_be_bytes(head[0..4].try_into().unwrap());
        let opcode = u32::from_be_bytes(head[4..8].try_into().unwrap());
        let uid = u32::from_be_bytes(head[8..12].try_into().unwrap());
        let len = u32::from_be_bytes(head[12..16].try_into().unwrap()) as usize;
        // Guard against a desynchronised stream asking us to allocate wildly.
        if len > 64 << 20 {
            bail!("implausible VI Server message length {len}; stream out of sync");
        }
        let mut body = vec![0u8; len];
        self.sock.read_exact(&mut body).context("reading VI Server body")?;
        Ok(Message { err, opcode, uid, body })
    }

    /// Send a request and wait for the reply carrying the same uID.
    fn request(&mut self, opcode: u32, body: &[u8], expect: u32) -> Result<Message> {
        let uid = self.take_uid();
        self.send(opcode, uid, body)?;
        loop {
            let reply = self.recv()?;
            if reply.uid != uid {
                // Another reply in flight (the protocol allows pipelining).
                continue;
            }
            if reply.err != 0 {
                bail!("VI Server returned error {}{}", reply.err, describe_error(reply.err));
            }
            if reply.opcode != expect {
                bail!("expected reply opcode {expect}, got {}", reply.opcode);
            }
            return Ok(reply);
        }
    }

    /// Resolve a VI path to a reference. Loading the VI is what makes LabVIEW
    /// resolve its links, so this is where the time goes on a cold VI.
    pub fn open_vi_reference(&mut self, path: &Path) -> Result<VIRef> {
        let pth0 = encode_pth0(path)?;
        let mut body = Vec::with_capacity(pth0.len() + 20);
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&GET_VI_REF_FLAGS.to_be_bytes());
        body.extend_from_slice(&(pth0.len() as u32).to_be_bytes());
        body.extend_from_slice(&pth0);
        body.extend_from_slice(&[0u8; 8]); // trailing options; zero as observed

        let reply = self.request(OP_GET_VI_REF, &body, RET_GET_VI_REF)?;
        if reply.body.len() < 4 {
            bail!("GetVIRef reply too short ({} bytes)", reply.body.len());
        }
        let refnum = u32::from_be_bytes(reply.body[0..4].try_into().unwrap());
        Ok(VIRef(refnum))
    }

    /// Call a VI by reference, passing pre-flattened arguments.
    ///
    /// Arguments are LabVIEW flattened data matching the target VI's connector
    /// pane, so they are the caller's business — this only frames them. That is
    /// exactly why we call our own VIs: their panes are ours to define, so the
    /// bytes stay under our control instead of tracking someone else's.
    #[allow(dead_code)]
    pub fn call(&mut self, vi: VIRef, args: &[u8]) -> Result<Vec<u8>> {
        let mut body = Vec::with_capacity(args.len() + 4);
        body.extend_from_slice(&vi.0.to_be_bytes());
        body.extend_from_slice(args);
        let reply = self.request(OP_CALL, &body, RET_CALL)?;
        Ok(reply.body)
    }

    /// Invoke a VI-class method.
    ///
    /// Body layout, confirmed against LabVIEW's logged `viRef=`/`meth=` fields:
    /// `+0 refnum | +4 format tag (2) | +8 method id | +12 flattened arguments`.
    pub fn invoke<M: Method>(&mut self, vi: VIRef, method: &M) -> Result<M::Output> {
        let args = method.encode_args()?;
        let mut body = Vec::with_capacity(12 + args.len());
        body.extend_from_slice(&vi.0.to_be_bytes());
        body.extend_from_slice(&ARGS_FORMAT.to_be_bytes());
        body.extend_from_slice(&M::ID.to_be_bytes());
        body.extend_from_slice(&args);
        let reply = self
            .request(OP_VI_DO_METHOD, &body, RET_VI_DO_METHOD)
            .with_context(|| format!("invoking {} on {vi}", M::NAME))?;
        method.decode_reply(&reply.body)
    }

    /// Run a VI and wait for it to finish, keeping the reference alive so its
    /// front panel can still be read afterwards. Remember the read timeout
    /// must cover the VI's runtime. See [`RunVi`] for the other combinations.
    pub fn run_vi(&mut self, vi: VIRef) -> Result<()> {
        self.invoke(vi, &RunVi { wait_until_done: true, auto_dispose_ref: false })
    }

    /// Start a VI and return as soon as LabVIEW acknowledges the request,
    /// leaving it running. The reference is kept (auto dispose off), which is
    /// what makes the front panel readable while it works — `ctrl_val_get`
    /// against a still-running VI is how progress gets out of a long job
    /// without the caller blocking for the whole runtime.
    ///
    /// Only the `Wait until done` byte differs from [`Connection::run_vi`];
    /// everything else is the same captured invocation.
    ///
    /// Two consequences worth knowing. The read timeout no longer has to cover
    /// the VI's runtime, only a round trip. And nothing here reports completion
    /// — the VI needs a front-panel flag of its own for that, since a running
    /// VI answers polls exactly as a finished one does.
    pub fn run_vi_async(&mut self, vi: VIRef) -> Result<()> {
        self.invoke(vi, &RunVi { wait_until_done: false, auto_dispose_ref: false })
    }

    /// Save a VI to a path — an ordinary in-place save. See [`SaveInstrument`].
    pub fn save_instrument(&mut self, vi: VIRef, path: &Path) -> Result<()> {
        self.invoke(vi, &SaveInstrument { path })
    }

    /// Write a front-panel control by label. See [`CtrlValSet`].
    pub fn ctrl_val_set(&mut self, vi: VIRef, control: &str, value: LvValue) -> Result<()> {
        self.invoke(vi, &CtrlValSet { control, value })
    }

    /// Read one half of a front panel: the controls when `controls` is true,
    /// the indicators when it is false. See [`CtrlValGetAll`] — the method's
    /// name promises more than it delivers.
    pub fn ctrl_val_get_all(&mut self, vi: VIRef, controls: bool) -> Result<Vec<(String, LvValue)>> {
        self.invoke(vi, &CtrlValGetAll { controls })
    }

    /// Read a whole front panel — controls *and* indicators, in that order.
    ///
    /// Two calls, because `Ctrl Val.Get All` selects one half at a time. Labels
    /// are unique within a panel, so the two halves cannot collide.
    pub fn ctrl_val_get_panel(&mut self, vi: VIRef) -> Result<Vec<(String, LvValue)>> {
        let mut out = self.ctrl_val_get_all(vi, true)?;
        out.extend(self.ctrl_val_get_all(vi, false)?);
        Ok(out)
    }

    /// Read one front-panel object by label. See [`CtrlValGet`].
    pub fn ctrl_val_get(&mut self, vi: VIRef, control: &str) -> Result<LvValue> {
        self.invoke(vi, &CtrlValGet { control })
    }

    /// Release a reference. LabVIEW sends no reply, so this must not wait.
    /// The reference itself travels in the uID field rather than the body.
    pub fn release(&mut self, vi: VIRef) -> Result<()> {
        self.send(OP_RELEASE_REF, vi.0, &[])
    }

    /// Say goodbye. Best-effort: a failure here cannot affect work already done.
    pub fn close(mut self) {
        let _ = self.send(OP_BYE, 0, &[]);
    }
}

/// A VI reference granted by LabVIEW.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VIRef(pub u32);

impl std::fmt::Display for VIRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#010x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The address at +28 is the only field of the handshake that varies, and
    /// LabVIEW rejects a wrong one with 1379 — so pin where it lands and that
    /// nothing else moves with it.
    #[test]
    fn hello_payload_carries_our_own_address() {
        let p = hello_payload(Ipv4Addr::LOCALHOST);
        assert_eq!(&p[28..32], &[127, 0, 0, 1]);
        assert_eq!(&p[0..4], &[0x26, 0x00, 0x80, 0x00], "version stamp");
        assert_eq!(&p[20..28], b"(Nobody)", "unauthenticated user name");
        // Beyond the length declared at +4 (0x1c = 28); LabVIEW ignores it.
        assert_eq!(&p[32..36], &[0, 0, 0, 0]);

        let q = hello_payload(Ipv4Addr::new(192, 168, 2, 11));
        assert_eq!(&q[28..32], &[192, 168, 2, 11]);
        assert_eq!(p[..28], q[..28], "only the address field may vary");
        assert_eq!(p[32..], q[32..], "only the address field may vary");
    }

    #[test]
    fn encodes_a_windows_path_as_pth0() {
        let got = encode_pth0(Path::new(r"C:\Git\lvpm\tools\Set VI Server Logging.vi")).unwrap();
        // Byte-for-byte the record LabVIEW's own client sent for this path.
        let want: &[u8] = &[
            b'P', b'T', b'H', b'0', 0, 0, 0, 0x2e, 0, 0, 0, 5, 1, b'C', 3, b'G', b'i', b't', 4,
            b'l', b'v', b'p', b'm', 5, b't', b'o', b'o', b'l', b's', 0x18, b'S', b'e', b't', b' ',
            b'V', b'I', b' ', b'S', b'e', b'r', b'v', b'e', b'r', b' ', b'L', b'o', b'g', b'g',
            b'i', b'n', b'g', b'.', b'v', b'i',
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn forward_slashes_and_empty_components_are_tolerated() {
        let a = encode_pth0(Path::new(r"C:\Git\lvpm")).unwrap();
        let b = encode_pth0(Path::new("C:/Git//lvpm")).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn rejects_an_empty_path() {
        assert!(encode_pth0(Path::new("")).is_err());
    }

    /// The exact 194-byte argument block LabVIEW's own client sent when saving
    /// this VI. If our encoder drifts from it, this fails.
    #[test]
    fn save_args_match_the_captured_invocation() {
        let got = SaveInstrument { path: Path::new(r"C:\Git\lvpm\tools\Set VI Server Logging.vi") }
            .encode_args()
            .unwrap();
        let want: &[u8] = b"\x00\x00\x00\x04\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x10\
\x00\x00\x00\x5e\x00\x00\x00\x24\x00\x00\x00\x01\x00\x1c\x40\x32\xff\xff\xff\xff\
\x12Path to saved file\x00\x00\x01\x00\x00PTH0\x00\x00\x00\x2e\x00\x00\x00\x05\
\x01C\x03Git\x04lvpm\x05tools\x18Set VI Server Logging.vi\
\x00\x00\x00\x10\x00\x00\x00\x1e\x00\x00\x00\x18\x00\x00\x00\x01\x00\x10\x40\x21\
\x0bSave a Copy\x00\x01\x00\x00\x00\x00\x00\x00\
\x00\x10\x00\x00\x00\x22\x00\x00\x00\x1c\x00\x00\x00\x01\x00\x14\x40\x21\
\x0fWithout Diagram\x00\x01\x00\x00\x00\x00";
        assert_eq!(got.len(), 194, "argument block length");
        assert_eq!(got, want);
    }

    /// A longer path must only change the path parameter's block size.
    #[test]
    fn save_args_block_size_tracks_path_length() {
        let args = |p| SaveInstrument { path: Path::new(p) }.encode_args().unwrap();
        let short = args(r"C:\ab\cd.vi");
        let long = args(r"C:\ab\cdcdcdcdcd.vi");
        let size_of = |b: &[u8]| u32::from_be_bytes(b[16..20].try_into().unwrap());
        assert_eq!(size_of(&long) - size_of(&short), (long.len() - short.len()) as u32);
    }

    /// The hook pattern — wait for the VI, keep the reference alive — as
    /// captured from the E2E test after the auto-dispose bug was fixed.
    #[test]
    fn run_vi_matches_the_captured_wait_no_dispose_invocation() {
        let got = RunVi { wait_until_done: true, auto_dispose_ref: false }.encode_args().unwrap();
        let want: &[u8] = b"\x00\x00\x00\x03\x00\x00\x00\x01\x00\x00\x00\x00\
\x00\x00\x00\x10\x00\x00\x00\x22\x00\x00\x00\x1c\x00\x00\x00\x01\x00\x14\x40\x21\
\x0fWait until done\x00\x01\x00\x00\x01\x00\
\x00\x00\x00\x10\x00\x00\x00\x24\x00\x00\x00\x1e\x00\x00\x00\x01\x00\x16\x40\x21\
\x10Auto Dispose Ref\x00\x00\x01\x00\x00\x00\x00";
        assert_eq!(got, want);
    }

    /// `Ctrl Val.Set` with a Dbl, an I32 and a Boolean — each byte-for-byte
    /// the block LabVIEW's own client sent for the same control and value.
    #[test]
    fn ctrl_val_set_matches_the_captured_invocations() {
        let cases: [(&str, LvValue, &[u8]); 3] = [
            (
                "Double in",
                LvValue::Dbl(125.0),
                b"\x00\x00\x00\x03\x00\x00\x00\x01\x00\x00\x00\x00\
\x00\x00\x00\x10\x00\x00\x00\x30\x00\x00\x00\x1e\x00\x00\x00\x01\x00\x16\x40\x30\
\xff\xff\xff\xff\x0cControl Name\x00\x00\x01\x00\x00\x00\x00\x00\x09Double in\x00\
\x00\x00\x00\x10\x00\x00\x00\x34\x00\x00\x00\x12\x00\x00\x00\x01\x00\x0a\x40\x53\
\x05Value\x00\x01\x00\x00\
\x26\x00\x80\x00\x00\x00\x00\x01\x00\x05\x00\x0a\x00\x00\x01\x00\x00\
\x40\x5f\x40\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
            ),
            (
                "Int32 in",
                LvValue::I32(125),
                b"\x00\x00\x00\x03\x00\x00\x00\x01\x00\x00\x00\x00\
\x00\x00\x00\x10\x00\x00\x00\x2e\x00\x00\x00\x1e\x00\x00\x00\x01\x00\x16\x40\x30\
\xff\xff\xff\xff\x0cControl Name\x00\x00\x01\x00\x00\x00\x00\x00\x08Int32 in\
\x00\x00\x00\x10\x00\x00\x00\x30\x00\x00\x00\x12\x00\x00\x00\x01\x00\x0a\x40\x53\
\x05Value\x00\x01\x00\x00\
\x26\x00\x80\x00\x00\x00\x00\x01\x00\x05\x00\x03\x00\x00\x01\x00\x00\
\x00\x00\x00\x7d\x00\x00\x00\x00\x00",
            ),
            (
                "Boolean in",
                LvValue::Bool(true),
                b"\x00\x00\x00\x03\x00\x00\x00\x01\x00\x00\x00\x00\
\x00\x00\x00\x10\x00\x00\x00\x30\x00\x00\x00\x1e\x00\x00\x00\x01\x00\x16\x40\x30\
\xff\xff\xff\xff\x0cControl Name\x00\x00\x01\x00\x00\x00\x00\x00\x0aBoolean in\
\x00\x00\x00\x10\x00\x00\x00\x2c\x00\x00\x00\x12\x00\x00\x00\x01\x00\x0a\x40\x53\
\x05Value\x00\x01\x00\x00\
\x26\x00\x80\x00\x00\x00\x00\x01\x00\x04\x00\x21\x00\x01\x00\x00\x01\x00\x00\x00\x00\x00",
            ),
        ];
        for (control, value, want) in cases {
            let got = CtrlValSet { control, value }.encode_args().unwrap();
            assert_eq!(got, want, "Ctrl Val.Set {control:?}");
        }
    }

    /// A string value has no capture with an unnamed descriptor (the E2E VI's
    /// flattened control carried its label), so check the derived layout.
    #[test]
    fn ctrl_val_set_string_uses_the_derived_layout() {
        let got = CtrlValSet { control: "String in", value: LvValue::Str("Hello".into()) }
            .encode_args()
            .unwrap();
        // The variant: version, one descriptor (unnamed String), selector,
        // u32-length string, no attributes, plus one byte of block padding.
        let tail: &[u8] = b"\x26\x00\x80\x00\x00\x00\x00\x01\x00\x08\x00\x30\xff\xff\xff\xff\
\x00\x01\x00\x00\x00\x00\x00\x05Hello\x00\x00\x00\x00\x00";
        assert!(got.ends_with(tail), "variant layout drifted");
    }

    /// `Ctrl Val.Get All` including indicators, byte-for-byte the captured
    /// request. The return-type section is itself from this capture, so what
    /// this really pins down is the framing and the Controls parameter.
    #[test]
    fn ctrl_val_get_all_matches_the_captured_invocation() {
        let got = CtrlValGetAll { controls: false }.encode_args().unwrap();
        let mut want = vec![0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x21, 0x00, 0x00, 0x00, 0x62];
        want.extend_from_slice(&GET_ALL_RETURN_TYPES);
        want.extend_from_slice(
            b"\x00\x00\x00\x10\x00\x00\x00\x1c\x00\x00\x00\x16\x00\x00\x00\x01\
\x00\x0e\x40\x21\x08Controls\x00\x00\x01\x00\x00\x00\x00",
        );
        assert_eq!(got, want);
    }

    /// The captured 366-byte `Ctrl Val.Get All` reply: four named variants —
    /// Dbl, I32, Boolean, String — after the E2E VI incremented its inputs.
    #[test]
    fn ctrl_val_get_all_decodes_the_captured_reply() {
        let reply: &[u8] = &{
            let mut r = vec![0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x21, 0x00, 0x00, 0x01, 0x40];
            r.extend_from_slice(&GET_ALL_RETURN_TYPES);
            // Array of four (Name, Variant Data) pairs...
            r.extend_from_slice(b"\x00\x00\x00\x04");
            r.extend_from_slice(b"\x00\x00\x00\x0aDouble out\x26\x00\x80\x00\x00\x00\x00\x01\
\x00\x11\x40\x0a\x00\x0aDouble out\x00\x00\x01\x00\x00\x40\x5f\x80\x00\x00\x00\x00\x00\x00\x00\x00\x00");
            r.extend_from_slice(b"\x00\x00\x00\x09Int32 out\x26\x00\x80\x00\x00\x00\x00\x01\
\x00\x0f\x40\x03\x00\x09Int32 out\x00\x01\x00\x00\x00\x00\x00\x7e\x00\x00\x00\x00");
            r.extend_from_slice(b"\x00\x00\x00\x0bBoolean out\x26\x00\x80\x00\x00\x00\x00\x01\
\x00\x10\x40\x21\x0bBoolean out\x00\x01\x00\x00\x00\x00\x00\x00\x00");
            r.extend_from_slice(b"\x00\x00\x00\x0aString out\x26\x00\x80\x00\x00\x00\x00\x01\
\x00\x14\x40\x30\xff\xff\xff\xff\x0aString out\x00\x00\x01\x00\x00\
\x00\x00\x00\x0dHello_aString\x00\x00\x00\x00");
            // ...then the echoed Controls parameter, value stripped.
            r.extend_from_slice(b"\x00\x00\x00\x10\x00\x00\x00\x1a\x00\x00\x00\x16\x00\x00\x00\x01\
\x00\x0e\x40\x21\x08Controls\x00\x00\x01\x00\x00");
            assert_eq!(r.len(), 366, "reply reconstruction");
            r
        };
        let got = CtrlValGetAll { controls: false }.decode_reply(reply).unwrap();
        assert_eq!(
            got,
            vec![
                ("Double out".to_string(), LvValue::Dbl(126.0)),
                ("Int32 out".to_string(), LvValue::I32(126)),
                ("Boolean out".to_string(), LvValue::Bool(false)),
                ("String out".to_string(), LvValue::Str("Hello_aString".into())),
            ]
        );
    }

    /// `Ctrl Val.Get` with the return type declared, byte-for-byte the request
    /// LabVIEW's own client sent once the output terminal was wired.
    #[test]
    fn ctrl_val_get_matches_the_captured_invocation() {
        let got = CtrlValGet { control: "Double out" }.encode_args().unwrap();
        let want: &[u8] = b"\x00\x00\x00\x02\x00\x00\x00\x21\x00\x00\x00\x2a\
\x00\x00\x00\x26\x00\x00\x00\x01\x00\x1e\x40\x53\x19Get Control Value Variant\x00\x01\x00\x00\
\x00\x00\x00\x10\x00\x00\x00\x30\x00\x00\x00\x1e\x00\x00\x00\x01\x00\x16\x40\x30\
\xff\xff\xff\xff\x0cControl Name\x00\x00\x01\x00\x00\x00\x00\x00\x0aDouble out";
        assert_eq!(got, want);
    }

    /// Two captured `Ctrl Val.Get` replies: the variant sits between the
    /// echoed return-type section and the parameter echo, padded to even.
    #[test]
    fn ctrl_val_get_decodes_the_captured_replies() {
        let head: &[u8] = b"\x00\x00\x00\x02\x00\x00\x00\x21";
        let types: &[u8] = b"\x00\x00\x00\x26\x00\x00\x00\x01\
\x00\x1e\x40\x53\x19Get Control Value Variant\x00\x01\x00\x00";
        let echo: &[u8] = b"\x00\x00\x00\x10\x00\x00\x00\x22\x00\x00\x00\x1e\x00\x00\x00\x01\
\x00\x16\x40\x30\xff\xff\xff\xff\x0cControl Name\x00\x00\x01\x00\x00";
        let cases: [(&[u8], u8, LvValue); 2] = [
            (
                // "Double out" = 126.0, one pad byte after the attributes.
                b"\x26\x00\x80\x00\x00\x00\x00\x01\x00\x11\x40\x0a\x00\x0aDouble out\x00\
\x00\x01\x00\x00\x40\x5f\x80\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
                0x54,
                LvValue::Dbl(126.0),
            ),
            (
                // "Boolean out" = false.
                b"\x26\x00\x80\x00\x00\x00\x00\x01\x00\x10\x40\x21\x0bBoolean out\
\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00",
                0x4c,
                LvValue::Bool(false),
            ),
        ];
        for (data, section, want) in cases {
            let mut reply = head.to_vec();
            reply.extend_from_slice(&(section as u32).to_be_bytes());
            reply.extend_from_slice(types);
            reply.extend_from_slice(data);
            reply.extend_from_slice(echo);
            let got = CtrlValGet { control: "x" }.decode_reply(&reply).unwrap();
            assert_eq!(got, want);
        }
    }

    /// Values of odd flattened length must pad to even inside a parameter
    /// block, keeping the block walkable.
    #[test]
    fn parameter_values_pad_to_even_length() {
        let p = param(type_desc(TD_BOOL, Some("Controls")).unwrap(), &[1]);
        assert_eq!(p.len() % 2, 0);
        let blk = u32::from_be_bytes(p[4..8].try_into().unwrap()) as usize;
        assert_eq!(p.len(), 8 + blk);
        assert_eq!(&p[p.len() - 2..], &[1, 0]);
    }

    #[test]
    fn unsupported_values_refuse_to_encode() {
        let v = LvValue::Other { code: 0x08, data: vec![0; 8] };
        assert!(CtrlValSet { control: "x", value: v }.encode_args().is_err());
    }
}
