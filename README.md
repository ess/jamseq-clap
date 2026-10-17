# `jamseq-clap`

**Standalone dynamic CLAP plugin host adapter for Jamband.**

---

## 1. Overview

`jamseq-clap` is an independent dynamic shared library (`libjamseq_clap.so` on Linux, `libjamseq_clap.dylib` on macOS, and `jamseq_clap.dll` on Windows) that implements the binary-stable `JamSeqAdapterVTable` C-ABI contract defined by `jamseq-plugin-api`.

It completely isolates `clack-host`, `clack-extensions`, and CLAP C bindings from the core `jamseq` audio engine.

---

## 2. Capabilities

- **Discovery & Scanning**: Scans standard system CLAP directories (`/usr/lib/clap`, `~/.clap`, `CLAP_PATH`, etc.) and automatically categorizes plugins into `Instrument`, `AudioEffect`, or `Analyzer`.
- **Instance Lifecycle & Real-Time DSP**: Sample-accurate block rendering with multi-channel audio buffers and frame-accurate MIDI events.
- **Parameter Introspection & Modulation**: Full enumeration of plugin parameters, value formatting, and real-time updates.
- **Note Name Introspection**: Introspects mapped drum and note names via `clap.note-name`, with automated fallback to the General MIDI Level 1 drum standard for percussion instruments.
- **Dynamic C-ABI Export**: Exports `jamseq_get_host_adapter` to enable dynamic loading via `libloading`.

---

## 3. Verification

Run the test suite:
```bash
cargo test -p jamseq-clap
```
