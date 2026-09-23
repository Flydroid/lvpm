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
| 0 | `ClientSaysHeaveno` | 10 | handshake; the reply's uID is a version stamp, not our id |
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

36-byte payload, sent with `uID = 6`. It embeds a version stamp, the user name
(`(Nobody)` when unauthenticated) and the client's own host address.

```
26 00 80 00  00 00 00 1c  1c 00 00 00  29 00 00 00
^^ version   ^^ length 28
08 00 00 00  "(Nobody)"   c0 a8 02 0b  00 00 00 00
                          ^^ host IP   ^^ past the declared length; ignored
```

Only two fields matter, and both were established by bisecting single fields
against a live 2026 server rather than by replaying a capture:

**The host address at +28 is validated.** It must be an address the client
machine actually holds. `127.0.0.1` and each of the host's interface addresses
handshake with `err=0`; `0.0.0.0`, and any address the machine does not hold,
come back **1379** — after which the server sends FIN and drops the connection.
This is why it cannot be a constant: the address originally captured here was a
DHCP lease, and the handshake broke silently the day the lease moved. Take it
from the connected socket (`TcpStream::local_addr`), which is right for loopback
and stays right against a remote LabVIEW.

**Bytes +32..+35 are padding.** They fall beyond the length declared at +4
(`0x1c` = 28). All-zeros, all-ones and two separately observed values are all
accepted, so send zeros rather than replaying a capture's uninitialised bytes.

**A rejected handshake still replies with opcode 10**, carrying the error in the
usual `err` field, and only then closes. A client that checks the opcode but not
`err` sails past the rejection and fails later on an unrelated read — the visible
symptom being a connection reset with no obvious cause. Check both.

### The version stamp

The stamp at +0 is LabVIEW's major/minor/fix/stage encoding — `0x26008000` is
2026 — and the same encoding appears on every flattened variant.

The server does **not** report its own version back: the hello reply's `uID`
echoes whatever stamp the client sent (claim `0x15008000` and `0x15008000` comes
back), so it is not a passive version oracle. But sending `0x00000000` is
rejected with **1037**, and *that* reply carries the server's real stamp in its
`uID`. So the server's version can be probed with one throwaway connection.

A 2026 server accepted clients stamping themselves 2025 and 2015, both `err=0`.
The stamp is tolerated downward, so one old stamp may serve several targets
rather than needing a table per release.

The *handshake* stamp is in fact tolerated in **both** directions: a 2025 server
accepts a client claiming 2026 with `err=0`, measured against a live v25.3. So
the handshake is not where a version mismatch shows up.

### Flattened data is checked, and only downward

The same encoding stamps every flattened variant, and there the tolerance runs
one way only. A server unflattens data stamped at or below its own version and
rejects anything above it with **122** — *"the resource you are attempting to
open was created in a more recent version of LabVIEW and is incompatible with
this version"*.

This is easy to misread as a broken VI rather than a wire problem, because the
error names a *resource*. Measured against a live 2025 server: a `Ctrl Val.Set`
carrying a variant stamped `0x26008000` fails with 122 while the identical call
stamped `0x20008000` succeeds. Relinking a folder begins by setting the folder
control, so a too-new stamp fails every package on an older LabVIEW while
working perfectly on the newest one.

Stamp flattened data at the oldest LabVIEW supported (`0x20008000`, 2020), not
at the newest seen.

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
`0x03` I32, `0x0a` Dbl, `0x40` Array, `0x50` Cluster — matching `LvTypeCode`
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
u32 count          type descriptors; 1 for a scalar, 2 for an array
<type descriptor>  unnamed when a client flattens a bare value; named with the
                   control's label when LabVIEW flattens a front-panel object
00 01 | u16 root   one top-level type, and which descriptor the data
                   conforms to: 0 for a scalar, 1 for an array (see below)
<value>
u32 attributes     0 in everything observed
```

Variants carry no padding of their own — any padding belongs to the enclosing
parameter block. Observed: Dbl `40 5F 40 00…` = 125.0, I32 `00 00 00 7d`
= 125, Boolean `01` = TRUE, String `u32 len + "Hello"`.

#### Arrays — a second descriptor and a root index

A variant whose value is an array sends **two** descriptors: the element's,
then the array's, which names no element type of its own but points back into
the same table by index. The trailer after the table is not a constant — the
second u16 is the index of the descriptor the data conforms to:

```
u16 len | u16 flags|0x40    array; 0x40 flag = named
u16 dims                    dimension count
u32 * dims                  each dimension's size; ffffffff = variable
u16 elem                    index of the element's descriptor in the table
[pascal name, pad to even]
```

Captured for a two-element string array (`String Array out` on the E2E test
VI), with `00 01 00 01` where a scalar sends `00 01 00 00`:

```
26008000 00000002
  0016 4030 ffffffff 0c "String out 2" 00        element: variable-size string
  001e 4040 0001 ffffffff 0000 10 "String Array out" 00
                                                 array: 1 dim, variable, elem=0
00 01 00 01                                      one top-level type: index 1
00000002                                         element count
00000007 "Hello 1" 00000007 "Hello 2"            packed, no padding between
00000000                                         attributes
```

The element descriptor's **name is stale** — LabVIEW ignores it. The Get reply
above carried `"String out 2"` (a different control), and the Set request
carried `"Control Name"`; the array descriptor holds the real label. Unnamed
element descriptors are accepted, and are what lvpm writes.

Because the array descriptor is element-agnostic, a Dbl or Boolean array
differs only in the element descriptor and the flattened data — unnamed, the
array descriptor is the same twelve bytes every time
(`000c 0040 0001 ffffffff 0000`). Element data packs with no per-element
header beyond what the element type already carries: strings keep their `u32`
length, numerics are bare big-endian, Booleans one byte each. String and I32
elements are captured; Dbl and Boolean array elements are derived from them
and from the scalar descriptors in the same captures, and
`numeric_arrays_reuse_the_string_array_framing` in `src/viserver.rs` pins that
derivation.

#### More than one dimension

`dims` really is a count, and the data carries one `u32` length per dimension
before the elements, row-major. Captured from `Int32 2D Array out` holding
`[[2, 3], [12, 23]]`:

```
26008000 00000002
  0019 4003 00 12 "Int32 2D Array out" 00        element: I32
  0024 4040 0002 ffffffff ffffffff 0000 12 "Int32 2D Array out" 00
                                                 array: 2 dims, both variable
00 01 00 01
00000002 00000002                                one length per dimension
00000002 00000003 0000000c 00000017              row-major
00000000
```

The matching **Set**, sent by LabVIEW's own client for `Int32 2D Array in`, is
the useful one: it shows what a *client* is expected to write, and it writes
the array descriptor **unnamed** — `0010 0040 0002 ffffffff ffffffff 0000` —
naming only the element (`000d 4003 00 07 "Numeric"`, a label LabVIEW then
ignores). lvpm writes both descriptors unnamed and LabVIEW accepts it.

### Reading values back — declare the return type

**A method only returns data if the request declares the type it expects.**
LabVIEW's own client derives that from the diagram: with the method's output
terminal unwired it sends flags `0x01` and no return-type section, and the
reply comes back err=0 with the parameter echo and *no value* — four such Get
replies were byte-identical 54-byte shells, which looked like "Get doesn't
work over TCP" until the terminal was wired. Wired, the same call sends flags
`0x21` plus a return-type section and the value comes back.

`Ctrl Val.Get` declares a single variant named after its output terminal:

```
u32 38 | u32 1 | td: Variant "Get Control Value Variant" | 00 01 00 00
```

`Ctrl Val.Get All` declares an array of cluster{`Name`: String, `Variant
Data`: Variant} — a four-entry descriptor table selected by index.

**`Ctrl Val.Get All` does not return everything.** Its single `Controls`
parameter is a *selector, not a filter*: TRUE returns the controls, FALSE the
indicators, and neither returns both. Confirmed by replaying one captured
request against a live 2026 server with only that value byte flipped, on a panel
holding four of each — TRUE gave exactly the four `… in`, FALSE exactly the four
`… out`. Reading a whole panel is two calls. (Watch the byte you flip: the
parameter's value is one byte padded to even length, so it sits at *−2* from the
end of the block, not −1.)

In the reply the declared section's length field grows to cover the returned
data, which sits between the return types and the parameter echo, padded to
even length as a whole (elements inside an array are not padded):

```
u32 slots | u32 flags | u32 types+data length | <return types as sent>
<data — Get: one variant; Get All: u32 count, then lv-string name + variant each>
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
| 1037 | version stamp not accepted | a zero stamp in the handshake; the reply's `uID` then carries the server's own |
| 1379 | claimed host address is not one this machine holds | handshake; the server closes the connection straight after |

The last two arrive on the *handshake* reply, which still uses opcode 10 — so a
client that only checks the opcode will miss them.

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

### Conformance against LabVIEW's own client

The strongest check available is a differential one, and it needs no new tooling:
capture LabVIEW's own client driving a VI, then drive the same VI with the same
values through `lvpm vi-run` and diff the request bodies past the refnum in
bytes 0..4. On 2026, over four `Ctrl Val.Set`, one `Run VI`, one `Ctrl
Val.Get All` and four `Ctrl Val.Get`, eight of ten were byte-identical. The two
that were not are both understood:

- `Run VI` differs in one byte — `Wait until done`, which `lvpm` deliberately
  sends TRUE where the captured diagram had the terminal wired FALSE.
- `Ctrl Val.Set` of a **string** differs by 14 bytes: LabVIEW names the variant's
  inner type descriptor (`00 16 40 30 ffffffff 0c "Control Name" 00`) where
  `lvpm` leaves it unnamed (`00 08 00 30 ffffffff`). The server accepts both —
  the value lands either way — so the name is optional here. Numeric and Boolean
  variants are byte-identical, so this is specific to the string case.

Two runs of the same client are byte-identical to each other once the refnum is
excluded, which is what makes the diff meaningful in the first place.

Note the failure mode this catches and `err` does not: a request can be accepted,
answered `err=0`, and still be wrong. A `Ctrl Val.Get` with no return-type
section, and a `Save:Instrument` that writes nothing, both return `err=0`. Every
live check needs an out-of-band oracle — the file's mtime, a value read back, the
panel's contents — never the error code alone.

The file format (`PTH0`, RSRC blocks) is a *different* risk class — far more
stable, since every VI ever saved depends on it, and byte-exactly verifiable
against known-good files.
