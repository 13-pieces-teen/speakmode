# Third-party release review — evidence gaps remain

This package provides collected evidence and reproducible staging, **not legal clearance or a finished release audit**. Collection did not install dependencies, run their scripts, download the WebView Runtime, or manipulate the running application.

## Current coverage

- Cargo.lock: 563 third-party packages; MSVC desktop/custom-protocol graph 353, GNU development graph 349; 246 distinct collected license/notice bodies. Runtime/build/proc-macro roles and direct dependencies are explicit in the inventory.
- 208 Cargo lock entries are outside both audited Windows graphs; their source/license availability is not a claim of inclusion in the Windows executable.
- npm lock: 139 entries, 83 installed runtime and 21 installed dev/build packages, 35 absent optional packages; 112 actual notice files. Optional absent entries have lock metadata only.
- 37 additional sections cover the Handy source extraction, pinned WebView2 SDK/Loader, bundled SQLite and matching MSVC Rust and GNU development compiler notices, plus the modified NSIS template original licenses.
- 5 exact MPL crate source archives are provided, checksum-verified against Cargo.lock. Preserve their source access alongside any applicable distribution.

## Three unresolved package texts

| Package | Scope | Missing evidence / required follow-up |
|---|---|---|
| realfft 3.5.0 | Cargo runtime candidate | MIT is declared; published VCS commit d0d4eee0525fd27c96c8a046d6d107acd5ed84a6 has no full license document located. Obtain an authoritative version-bound copyright/license text. |
| unicode-casefold 0.2.0 | Cargo runtime candidate | MIT/Apache-2.0 declared; published crate has no full license document or .cargo_vcs_info.json establishing its exact upstream revision. Obtain corresponding authoritative text. |
| react-remove-scroll-bar 2.3.8 | npm runtime | MIT declared; package has no full license file and exact npm gitHead/license/tree returned 404. Do not substitute current branch or a sibling package. |

The rolldown native build-package gap was recovered via an explicit archived npm registry-attestation subject/commit chain, fixed tag and Git source-blob hashes. Its gitHead remains null; signature verification and native tarball-byte verification were not performed. Both LICENSE and referenced THIRD-PARTY-LICENSE are retained. The native Tauri CLI 2.12.1 build package likewise has its own subject/commit-bound MIT, Apache and SPDX supplement; its source-tree API comparison was rate-limited and remains explicitly unverified. Raw files are pinned to the declared commit and SHA256 checked.

Exact retrieval attempts and available metadata remain in cargo/REVIEW.md, npm/REVIEW.md and their supplemental provenance. Missing text is not filled with a generic license or another release's copyright.

## Release boundary still to check

1. **Final MSVC artifact review:** the formal MSVC toolchain is available and project compilation/test results are separate evidence. This collection includes its matching Rust standard-library copyright materials. Cargo dependency graphs and those notices are not a final linked-binary, CRT redistribution, or installed-artifact audit. Compare actual shipped JS/native/CRT/runtime files before release. GNU materials remain labelled development-only.
2. **WebView2:** pinned SDK 1.0.3800.47 Loader notice is included separately from webview2-com MIT bindings. Current Tauri config requests downloadBootstrapper. Review the actual Evergreen Runtime/bootstrapper distribution terms and installer behavior before publishing. No Runtime or bootstrapper was downloaded here. Official reference: https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution .
3. **NSIS/installer and other assets:** these lockfiles do not inventory separately downloaded NSIS plugins, VC runtime redistributables, fonts, icons, or other future assets. Inspect the actual release bundle and corresponding notices; do not infer completeness from package counts.
4. **MPL/source provision:** included exact unmodified sources are a practical distribution input, not a legal judgment. If modified, retain notices and provide the corresponding modified source. Official license: https://www.mozilla.org/en-US/MPL/2.0/ .
5. **Conservative text inclusion:** nested crate notices, dual-license alternatives, compiler license directory files, and npm build-tool notices are retained. Inventory inclusion is not a claim that every listed license governs SpeakMode as a whole. Collection heuristics are evidence capture, not a complete transitive embedded-code audit.
6. **Packaging status:** Tauri resources are configured from `../../docs/third-party/nsis-resources/` to installed `third-party/`. The stage script copies/hash-checks the material and verifies that configuration. Actual installer contents, signing, installation, upgrade/uninstall behavior remain NOT_RUN in this receipt. `LOCAL-SOURCE-MODIFICATIONS.md` records the pinned modified NSIS template `/UPDATE` behavior and cross-version hooks dependency.

## Included source archives

- `cssparser@0.37.0`: `source-archives/cssparser-0.37.0.crate`; SHA256 `8c9cdaae01d5ed7882b04d795e7f752f46ff52d2fa3b50a20d28c464510bba98`; https://static.crates.io/crates/cssparser/cssparser-0.37.0.crate
- `cssparser-macros@0.7.1`: `source-archives/cssparser-macros-0.7.1.crate`; SHA256 `d045de693cb712d0b22c6a64be5b953f67b3ce00ab5ad3dd5d8b441886ab8e1a`; https://static.crates.io/crates/cssparser-macros/cssparser-macros-0.7.1.crate
- `dtoa-short@0.3.5`: `source-archives/dtoa-short-0.3.5.crate`; SHA256 `cd1511a7b6a56299bd043a9c167a6d2bfb37bf84a6dfceaba651168adfb43c87`; https://static.crates.io/crates/dtoa-short/dtoa-short-0.3.5.crate
- `option-ext@0.2.0`: `source-archives/option-ext-0.2.0.crate`; SHA256 `04744f49eae99ab78e0d5c0b603ab218f515ea8cfe5a456d7629ad883a3b6e7d`; https://static.crates.io/crates/option-ext/option-ext-0.2.0.crate
- `selectors@0.38.0`: `source-archives/selectors-0.38.0.crate`; SHA256 `8adfa1c298912827b8a28b223b3b874357397ae706e6190acd9bf28cee99114d`; https://static.crates.io/crates/selectors/selectors-0.38.0.crate
