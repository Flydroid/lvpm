#!/usr/bin/env python3
r"""Decode a VI Server TCP capture into request/reply pairs.

LabVIEW's built-in `Server:Logging Enabled` records only the messages LabVIEW
*receives* — every `kTS*Send`, never the `kTS*Return` reply. An npcap loopback
capture has both directions, so this recovers what the log cannot: the returned
refnums, property values and error codes.

    tshark -i \Device\NPF_Loopback -f "tcp port 3364" -w cap.pcapng
    python tools/decode_viserver_pcap.py cap.pcapng --port 3364

Wire framing (confirmed against the built-in log, big-endian):
    off 0  u32  err        (matches the log's err= field)
    off 4  u32  opcode     (Send opcodes as logged; Return = Send | 0x08-ish,
                            observed +10/+2 — see OPCODES; paired by uID)
    off 8  u32  uID        (request id; reply carries the request's uID)
    off 12 u32  len        (payload length after this 16-byte header)
    off 16 ...  payload
A single TCP segment may pack several messages; we walk by length.
"""

import argparse
import struct
import subprocess
from pathlib import Path

TSHARK = r"C:\Program Files\Wireshark\tshark.exe"

# Send opcodes from the log; Return opcodes observed in replies.
SEND = {
    0: "ClientSaysHeaveno", 1: "AppAttrVector", 2: "VIAttrVector", 3: "GetVIRef",
    4: "Call", 6: "VIDoMethod", 7: "ReleaseRef", 8: "ClientBye",
    22: "ObjAttrVector", 32: "Ping",
}
# Return opcodes are separate enum values, not a fixed offset from Send:
# small Send codes map +10 (1->11, 2->12, 3->13, 4->14, 6->16), but
# ObjAttrVector is 22->23 and Ping 32->33. Pinned from observation.
RETURN = {10: "ServerSaysHeaveno", 11: "AppAttrVector", 12: "VIAttrVector",
          13: "GetVIRef", 14: "Call", 16: "VIDoMethod", 23: "ObjAttrVector",
          33: "Ping"}

# kTSVIDoMethodSend body layout, confirmed against the built-in log:
#   +0  u32 target refnum      (log's viRef=)
#   +4  u32 argument count     (2 for FP.Open's Activate + State)
#   +8  u32 method id          (log's meth=; 1003 Run VI, 1080 FP.Open)
#   +12 ... flattened arguments
METHOD_ID_OFFSET = 8


def load_segments(pcap, port):
    """[(t, is_request, payload bytes)] in capture order."""
    out = subprocess.run(
        [TSHARK, "-r", str(pcap), "-Y", "tcp.len>0", "-T", "fields",
         "-e", "frame.time_relative", "-e", "tcp.dstport", "-e", "tcp.payload"],
        capture_output=True, text=True, check=True).stdout
    segs = []
    for line in out.splitlines():
        parts = line.split("\t")
        if len(parts) < 3 or not parts[2]:
            continue
        t = float(parts[0])
        is_req = parts[1] == str(port)
        segs.append((t, is_req, bytes.fromhex(parts[2].replace(":", ""))))
    return segs


def messages(payload):
    """Split one TCP payload into (err, opcode, uid, plen, body) messages."""
    off = 0
    while off + 16 <= len(payload):
        err, opcode, uid, plen = struct.unpack_from(">iIII", payload, off)
        body = payload[off + 16: off + 16 + plen]
        if len(body) < plen:  # message spans segments; stop, caller re-joins
            break
        yield err, opcode, uid, plen, body
        off += 16 + plen


def strings(b, minlen=4):
    out, cur = [], []
    for ch in b:
        if 0x20 <= ch < 0x7F:
            cur.append(ch)
        else:
            if len(cur) >= minlen:
                out.append(bytes(cur).decode("latin-1"))
            cur = []
    if len(cur) >= minlen:
        out.append(bytes(cur).decode("latin-1"))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pcap", type=Path)
    ap.add_argument("--port", type=int, default=3364)
    ap.add_argument("--strings", action="store_true", help="show payload strings")
    a = ap.parse_args()

    segs = load_segments(a.pcap, a.port)
    # Re-join payloads split across segments per direction.
    buf = {True: b"", False: b""}
    events = []
    for t, is_req, data in segs:
        buf[is_req] += data
        consumed = 0
        for err, opcode, uid, plen, body in messages(buf[is_req]):
            events.append((t, is_req, err, opcode, uid, plen, body))
            consumed += 16 + plen
        buf[is_req] = buf[is_req][consumed:]

    reqs = sum(1 for e in events if e[1])
    reps = len(events) - reqs
    print(f"{a.pcap.name}: {len(events)} messages  ({reqs} requests, {reps} replies)\n")

    # Pair replies to requests by uID.
    pending = {}
    paired = unpaired = 0
    for t, is_req, err, opcode, uid, plen, body in events:
        if is_req:
            name = SEND.get(opcode, f"op{opcode}")
            pending[uid] = (t, name, plen, body)
            tag = f"REQ  {name:<18}"
            extra = ""
            if name == "GetVIRef":
                paths = [s for s in strings(body) if "\\" in s or "/" in s or s.endswith(".vi")]
                extra = f'  "{paths[0]}"' if paths else ""
            print(f"  {t:8.3f}  {tag} uID={uid:#010x} len={plen:<5}{extra}")
        else:
            name = RETURN.get(opcode, f"ret{opcode}")
            req = pending.pop(uid, None)
            if req:
                paired += 1
                dt = (t - req[0]) * 1000
                # Reply payload for GetVIRef starts with the granted refnum.
                ref = f"  ref={struct.unpack_from('>I', body)[0]:#010x}" if body and len(body) >= 4 else ""
                errtag = "" if err == 0 else f"  ERR={err}"
                print(f"  {t:8.3f}  REP  {name:<18} uID={uid:#010x} len={plen:<5}"
                      f"  (+{dt:.0f}ms){ref}{errtag}")
                if a.strings and body:
                    for s in strings(body)[:6]:
                        print(f"                  · {s}")
            else:
                unpaired += 1
                print(f"  {t:8.3f}  REP  {name:<18} uID={uid:#010x} len={plen:<5}  (unpaired)")

    print(f"\npaired {paired} replies to requests; {unpaired} replies had no "
          f"request in capture (connection predated capture start)")
    errs = [e for e in events if not e[1] and e[2] != 0]
    print(f"replies carrying a non-zero error: {len(errs)}"
          + ("" if not errs else "  <-- real outcomes the request-only log cannot show"))


if __name__ == "__main__":
    main()
