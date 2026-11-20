# Windows updater attribution supplement — SpeakMode 0.2.48

This is a focused addition to the existing frozen third-party materials, not a regeneration of their historical inventory or an assertion that the full current dependency graph has been audited. Existing provenance, counts, source archives and unresolved evidence gaps remain unchanged.

## Verified Windows delta

Compared with the 0.2.47 base Cargo.lock, the updater change adds 22 registry package/version entries. Intersecting those additions with the actual `x86_64-pc-windows-msvc`, `desktop`, normal-dependency graph selects the following three packages:

| Package | Declared license | Source |
| --- | --- | --- |
| tauri-plugin-updater 2.14.1 | Apache-2.0 OR MIT | [Versioned crate](https://crates.io/crates/tauri-plugin-updater/2.14.1); [upstream](https://github.com/tauri-apps/plugins-workspace) |
| minisign-verify 0.3.0 | MIT | [Versioned crate](https://crates.io/crates/minisign-verify/0.3.0); [upstream](https://github.com/jedisct1/rust-minisign-verify) |
| rustls-platform-verifier 0.7.1 | MIT OR Apache-2.0 | [Versioned crate](https://crates.io/crates/rustls-platform-verifier/0.7.1); [upstream](https://github.com/rustls/rustls-platform-verifier) |

The graph was resolved successfully with this read-only command (no compilation, network download or installer execution):

```text
cargo tree --locked --offline --manifest-path app/src-tauri/Cargo.toml --target x86_64-pc-windows-msvc --features desktop --edges normal --prefix none --format '{p}'
```

This comparison concerns newly locked packages. It does not claim that only these three packages differ from the older inventory's Windows snapshot, or re-audit dependencies already present in the base lock file whose features may have changed. Other new lock entries target macOS, Linux, Android or Wasm and were not selected in this Windows normal graph. Lock-file presence alone is not evidence that their code is shipped in the Windows executable.

## Exact license materials

The adjacent six license/SPDX files are byte-for-byte copies from the local published-crate sources. Every source file was compared with the same member inside the locally cached `.crate` archive, and each archive's SHA256 matched the current Cargo.lock checksum before copying. No third-party source was modified.

- **tauri-plugin-updater 2.14.1**: crate SHA256 `d957abbcb3f9f6c76d22a3a0f012f30880640d0bccb5634d2b12a9937e4c872b`; copied `tauri-plugin-updater-2.14.1-LICENSE.spdx`, `tauri-plugin-updater-2.14.1-LICENSE_APACHE-2.0`, `tauri-plugin-updater-2.14.1-LICENSE_MIT`.
- **minisign-verify 0.3.0**: crate SHA256 `871285dc19d8d0ebe0eef3d0e99a205f2a71363b122632cbbfa4a6c370a960ce`; copied `minisign-verify-0.3.0-LICENSE`.
- **rustls-platform-verifier 0.7.1**: crate SHA256 `1167586491e2b18b8bfbb293e8180ec17c201c4f076d7cb3070ca964e7598f98`; copied `rustls-platform-verifier-0.7.1-LICENSE-APACHE`, `rustls-platform-verifier-0.7.1-LICENSE-MIT`.

The distribution manifest adds hashes and byte sizes for this README and these six files. Its original `inputs` hashes and existing file entries remain the historical frozen snapshot; this supplement does not replace them. Package inclusion here is supported by the selected dependency graph, not by inspection of the final optimized binary.
