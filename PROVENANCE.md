# Provenance

lvpm implements the package model of the OpenG Package Tools (OGPT) and
interoperates with a package ecosystem whose dominant client today, JKI's VI
Package Manager (VIPM), is closed source. This document records where every
piece of format and protocol knowledge in this codebase came from, so nobody
has to wonder.

**lvpm contains no JKI code and no OpenG code.** Nothing in this repository
was produced by decompiling, disassembling, or otherwise extracting logic from
VIPM or any other JKI software. The OGPT source is LGPL and public, but lvpm
is a from-scratch Rust implementation of its *design documents*; no OGPT VI or
library is included or translated. Every format was learned from published
specifications, from reading data files, and from observing externally visible
behavior — the same things any user's text editor, zip tool, or network card
can see.

## The design: OpenG Package Tools

The package format, the package-directory format, the target-directory
keywords, the file-group and replace-mode semantics, the script-VI hook
points and the install/uninstall/upgrade sequences are all specified in the
OGPT design documents, published 2002–2005 at
<https://ogpm.sourceforge.net/design/OGPT%20Design%20TOC.html>, and in the
project's LGPL-2.0 source and CVS history at
<https://sourceforge.net/projects/ogpm/>. Those documents are lvpm's primary
reference; [docs/ogpm-model.md](docs/ogpm-model.md) summarises them and maps
each concept onto lvpm.

## Package format (`.ogp` / `.vip`)

An OGPT package is a plain zip archive containing a `spec` file — an
INI-format manifest — an icon, and `File Group N/` payload directories
(design docs "Archive Format" and "Package Spec File"). A `.vip` is the same
archive with a renamed `[Package]` section and a four-part version. The parser
in [src/spec.rs](src/spec.rs) was written from the spec-file design document
and checked against `spec` files from publicly downloadable packages. No tool
other than a zip reader and a text editor was needed.

## Package directory format (`.ogpd` / `.vipr`)

OGPM's Package Directory is a plain-text INI file, one
`[Package <name>-<version>-<release>]` section per entry with `Package.URL`,
`Spec.URL` and `Icon.URL` keys (design doc "Package Directories and
Repositories"; `openg.ogpd`, 2004, in the CVS snapshot). The public feeds
still use it; `.vipr` adds `Package.MD5` and a few `Package.*` metadata keys.
The parser in [src/index.rs](src/index.rs) was written from the format and
by reading those files. Note that the index endpoints themselves are operated
by NI and JKI respectively; fetching them is subject to those operators'
terms.

## Install semantics

What "installing a package" means — resolve each file group's `Target Dir`
keyword against the target LabVIEW, copy honouring `Replace Mode`, run the
`PreInstall` / `PostInstall` script VIs from the extraction directory — is
the OGPI installer sequence (design docs "OGPI Installer", "Path Roots").
VIPM's spelling of the keywords (`<vilib>`, `<userlib>`, the `<OS …>`
family) and the fact that palette `.mnu` files are copied rather than merged
were confirmed by snapshotting a LabVIEW installation tree before and after
package installs and diffing the two states. lvpm's installer was then
verified by comparing the filesystem state it produces against the state a
reference VIPM install produces: identical files, identical places.

## LabVIEW VI Server wire protocol

The TCP protocol spoken by [src/viserver.rs](src/viserver.rs) belongs to NI's
LabVIEW. NI publishes the VI Server *API* but not its wire format.
The format documented in [docs/vi-server-protocol.md](docs/vi-server-protocol.md)
was reconstructed for interoperability from two independent sources, both on
our own licensed LabVIEW installations: LabVIEW's own built-in server logging
facility (the `Server:Logging Enabled` application property), and loopback
packet captures of traffic between processes we ran ourselves. No NI binary
was decompiled or disassembled.

## What is deliberately kept out of this repository

Packet captures, install logs, and filesystem snapshots used during the
analysis are excluded by [.gitignore](.gitignore) and have never been
committed. They contain machine-specific paths and can contain credentials
(VIPM authentication tokens travel in cleartext in its HTTP traffic). Only the
conclusions are published, never the raw captures.

## Trademarks and affiliation

LabVIEW and NI are trademarks of National Instruments (Emerson). VIPM, VI
Package Manager, and JKI are trademarks of James Kring, Inc. OpenG and the
OpenG Package Tools are the work of the OpenG.org community. lvpm is an
independent project: it is not affiliated with, endorsed by, or supported by
NI, JKI or OpenG.org, and it ships none of their software.
