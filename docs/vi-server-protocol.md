# The LabVIEW VI Server TCP protocol

Everything here was **observed**, not taken from documentation — NI publishes the
VI Server *API*, never its wire format. Two independent sources agree on all of
it: LabVIEW's own `Server:Logging Enabled` output (which names messages and
methods) and npcap loopback captures (which carry both directions).

Verified against **LabVIEW 2026 (26.3) only**. See [Stability](#stability).

## Capturing it yourself

**LabVIEW's built-in log** — write the Application properties `Server:Log File
Path` then `Server:Logging Enabled` (in that order). Runtime-only: there is no
`LabVIEW.ini` token and no checkbox, so it resets on every LabVIEW restart. It
records only messages LabVIEW *receives* — replies never appear, so its `err`
field says nothing about whether an operation succeeded.

**Packet capture** — VI Server is loopback traffic, so an ordinary NIC capture
never sees it; npcap's loopback adapter is required.

```
dumpcap -i \Device\NPF_Loopback -f "tcp port 3364" -w cap.pcapng
python tools/decode_viserver_pcap.py cap.pcapng --port 3364
```

The port is per LabVIEW version — read `server.tcp.port` from that install's
`LabVIEW.ini` (2025 defaults to 3363, 2026 to 3364).

## Framing

All big-endian. A single TCP segment may pack several messages; walk by length.

```
+0   u32  err        0 from clients; on a reply, the operation's error code
+4   u32  opcode
+8   u32  uID        request id; the reply echoes it
+12  u32  len        payload length
+16  ...  payload
```

Replies pair to requests by `uID`. Client ids stride by `0x100000` with the low
20 bits normally clear. **Requests may be pipelined** — LabVIEW itself sends
several before any reply arrives — so a client must match on `uID` rather than
assume ordering.

## Opcodes

| Send | Name | Return | Notes |
|---:|---|---:|---|
| 0 | `ClientSaysHeaveno` | 10 | handshake; reply carries its own uID, not ours |
| 1 | `AppAttrVector` | 11 | Application property get/set |
| 2 | `VIAttrVector` | 12 | VI property get/set |
| 3 | `GetVIRef` | 13 | path → refnum |
| 4 | `Call` | 14 | Call By Reference |
| 6 | `VIDoMethod` | 16 | invoke a VI method |
| 7 | `ReleaseRef` | — | **no reply**; refnum travels in `uID`, body empty |
| 8 | `ClientBye` | — | no reply |
| 22 | `ObjAttrVector` | 23 | GObject/control property |
| 32 | `Ping` | 33 | keepalive |

Return opcodes are separate enum values, not an offset: most are send+10, but
`ObjAttrVector` is 22→23 and `Ping` 32→33.

`LabVIEW.exe` contains 61 `kTS*` symbols in Send/Return pairs; a package install
exercises only the ten above. Unused families include `kTSProj*` (projects),
`kTSLVTarget*` (targets) and `kTSCreateNewVI*` (scripting).

## Handshake

36-byte payload, sent with `uID = 6`. It embeds the user name (`(Nobody)` when
unauthenticated) and the host address, and the remaining fields are unexplained,
so `lvpm` replays it verbatim rather than guessing.

```
26 00 80 00  00 00 00 1c  1c 00 00 00  29 00 00 00
08 00 00 00  "(Nobody)"   c0 a8 02 fb  65 00 53 00
                          ^^ host IP
```

## Paths — `PTH0`

The same record appears in a VI file's `LIvi`/`LIbd` linker blocks, so one
encoder serves both the wire and any offline path rewriting.

```
"PTH0" | u32 byte length | u32 component count | pascal components...
```

An absolute Windows path contributes its drive letter as the first component:
`C:\Git\lvpm\tools\X.vi` → `01 "C" 03 "Git" 04 "lvpm" 05 "tools" 06 "X.vi"`.
The length field covers the count field plus the components.

## `GetVIRef` (3)

```
00000000 | 80200000 flags | u32 pth0 length | <PTH0> | 8 zero bytes
```

Reply body starts with the granted refnum. This is where the time goes — loading
the VI is what makes LabVIEW resolve its links (88–464 ms observed for a cold VI,
against 2–10 ms for a property read).

## `VIDoMethod` (6)

```
+0   u32  target refnum
+4   u32  format tag, always 2
+8   u32  method id
+12  ...  flattened arguments
```

The word at +4 is not an argument count: it is 2 whether the method takes one
parameter (`Ctrl Val.Get All`) or three (`Save:Instrument`).

### Method ids

Cross-checked against the built-in log, which names each one
(`meth=1003 (…context=lvcore_lvprop_vi_run_vi)`).

| Id | Method | Token |
|---:|---|---|
| 1002 | `Save:Instrument` | `lvcore_lvprop_vi_sve_instrument` |
| 1003 | `Run VI` | `lvcore_lvprop_vi_run_vi` |
| 1051 | `Ctrl Val.Set` | `lvcore_lvprop_vi_set_cont_valuevrnt` |
| 1052 | `Ctrl Val.Get` | `lvcore_lvprop_vi_get_cont_valuevrnt` |
| 1053 | `Ctrl Val.Get All` | `lvcore_lvprop_vi_get_all_cont_valuvrnt` |
| 1080 | `FP.Open` | `lvcore_lvprop_vi_open_front_panel` |

The `lvcore_lvprop_*` tokens sit in an id-ordered table inside `LabVIEW.exe`, but
it is **rank-ordered, not dense** — positions 143→154 span 11 tokens while ids
1053→1080 span 27. Position brackets an id; it never derives one.

### Arguments

Every length below derives from one rule set — nothing has to stay a captured
blob. `src/viserver.rs` generates all of it and pins each encoding
byte-for-byte against a capture of LabVIEW's own client.

```
u32 slots          parameter count + 1, counting the return slot
u32 flags          0x01, plus 0x20 when a return-type section follows
u32 length         of the return-type section; 0 when absent
[return types]
parameter blocks...
```

Each parameter block:

```
00000010                     block marker
u32 blk                      everything after this field
u32 tdsec                    the type-descriptor section
00000001                     descriptor count
<type descriptor>            named — the parameter's name travels here
00 01 00 00                  descriptor selector
<value>                      padded to even length
```

so `tdsec = 4 + td + 4` and `blk = 4 + tdsec + padded value`. An unwired
parameter is just `00000010 00000000`.

Type descriptors, length field included in itself:

```
u16 len | u16 flags|code     0x40 flag = named
[ffffffff]                   strings and paths only
[00]                         fixed-size numerics only (one reserved byte)
[pascal name, pad to even]
```

Codes seen: `0x21` Boolean, `0x30` String, `0x32` Path, `0x53` Variant,
`0x03` I32, `0x0a` Dbl, `0x40` Cluster, `0x50` Array — matching `LvTypeCode`
in the `Rust-LabVIEW-Interop` crate. Beware the dialect gap, though: that
crate's `typedesc` serializer targets in-memory *type strings*, whose real
LabVIEW golden vector encodes a named Dbl **without** the reserved byte
(`len=12`), while every numeric on the wire carries it (unnamed I32 =
`00 05 00 03 00`). Two contexts, one byte apart, both straight from LabVIEW.

Values flatten with no headers of their own: Boolean one byte, I32/Dbl
big-endian, strings as `u32 length + bytes`.

### Variants

`Ctrl Val.Set` takes `"Control Name"` (String) and `"Value"` (Variant). A
variant is flattened data with its descriptor inline:

```
u32 version        0x26008000 — LabVIEW 2026 release, in LabVIEW's own
                   major/minor/fix/stage encoding (the handshake reuses it);
                   older stamps are accepted
u32 count          type descriptors, 1
<type descriptor>  unnamed when a client flattens a bare value; named with the
                   control's label when LabVIEW flattens a front-panel object
00 01 00 00        descriptor selector
<value>
u32 attributes     0 in everything observed
```

Variants carry no padding of their own — any padding belongs to the enclosing
parameter block. Observed: Dbl `40 5F 40 00…` = 125.0, I32 `00 00 00 7d`
= 125, Boolean `01` = TRUE, String `u32 len + "Hello"`.

### Reading values back — Get vs Get All

**A method only returns data if the request declares the type it expects.**
LabVIEW's own client sends `Ctrl Val.Get` with flags `0x01` and no return-type
section, and the reply comes back err=0 with the parameter echo and *no value*
— all four observed Get replies are byte-identical 54-byte shells. `Ctrl
Val.Get All` sends flags `0x21` plus a return-type section declaring an array
of cluster{`Name`: String, `Variant Data`: Variant}, and its reply carries all
values. lvpm therefore reads single controls through Get All.

In the reply the declared section's length field grows to cover the returned
data, which sits between the return types and the parameter echo:

```
u32 slots | u32 flags | u32 types+data length | <return types as sent>
<data: u32 element count, then per element an lv-string name and a variant>
<parameter echoes, values stripped>
```

## Properties — `*AttrVector` (1, 2, 22)

The built-in log renders these only as `nAttrs=N`; the body has more:

```
+0   u32  refnum
+4   u32  0x02
+8   u32  attribute count
+12  u32  0x01
+16  u32  property id
+20  ...  value / type descriptor
```

Property ids are at `+16` (501 and 573 observed). The modification bitset used to
decide whether a VI needs saving lives here — tokens
`lvcore_lvprop_vi_modvi_modificat_bitset`, `..._modblockdgm_mods_bitset`,
`..._modfp_mods_bitset`.

## Errors

| Code | Meaning | When it bites |
|---:|---|---|
| 91 | variant type mismatch | wrong inner type in a `Ctrl Val.Set` |
| 1026 | VI Reference is invalid | see Auto Dispose below |
| 1031 | reference type ≠ connector pane | `LabVIEWCLI RunVI` on a VI with the wrong pane |
| 1032 | VI Server access denied | access list / `server.tcp.enabled` |

## Gotchas

**`Run VI` with *Auto Dispose Ref* = TRUE invalidates the reference.** LabVIEW
releases it the moment the VI finishes, so a following `Ctrl Val.Get` fails with
1026. For the set → run → get pattern, Auto Dispose must be FALSE — lvpm's
`run_vi` defaults to wait=TRUE, dispose=FALSE. Only those two value
bytes differ across all 24 captured `Run VI` blocks.

**With *Wait until done*, the reply arrives when the VI finishes** — the
socket's read timeout has to cover the VI's runtime, not just a round trip.

**There is no concurrency.** Parallel VI Server calls wait for each other, and
LabVIEW runs one VI Server per process — measured at ~96% of a *single* core
during a compile. Fanning out buys nothing; the throughput lever is skipping
saves for VIs that did not change.

**`ReleaseRef` gets no reply.** Waiting for one deadlocks.

**A save is not unconditional.** LabVIEW re-saves whatever it is told to, but
callers should gate on the modification bitset first — observed installs skip a
large share of files on that basis, one package rewriting none of its 385.

## Stability

These are internal enum values with no compatibility promise, and the id table is
not dense enough to derive from. Anything built on them needs re-validating per
LabVIEW release.

That is why `lvpm` keeps its dependency deliberately small — the five primitives
(hello, `GetVIRef`, `Call`, `ReleaseRef`, `ClientBye`) plus the handful of VI
methods above — and puts real logic in LabVIEW VIs written against the public,
documented API, which NI does keep stable. Re-check on a new version with:

```
lvpm vi-probe --labview-version <year> <some.vi>     # transport
python tools/decode_viserver_pcap.py new.pcapng      # opcode + id tables
```

The file format (`PTH0`, RSRC blocks) is a *different* risk class — far more
stable, since every VI ever saved depends on it, and byte-exactly verifiable
against known-good files.
