# Local source modification boundary

Updated 2026-10-23. This note accompanies the dependency inventory and is included in the staged
installer resource manifest. It is a source-modification record, not release/legal
approval or evidence of a successful installation.

## handy-keys 0.3.4

- Audited upstream: commit `e7d1b8e6de8258f21ee1d60608d16f8d195157bf`,
  <https://github.com/handy-computer/handy-keys/tree/e7d1b8e6de8258f21ee1d60608d16f8d195157bf>.
- Application source: `app/src-tauri/src/platform/vendor/handy-keys/`, selected by
  Cargo's local crates.io patch. It is a modified source tree, not a byte-identical
  upstream crate. The separate audited upstream checkout has not been edited.
- Original MIT license and attribution remain in the vendored `LICENSE`.
  No license text was changed by this repair.
- Previously documented changes provide actual hook/watcher constructor readiness,
  an ordered observed key/lifecycle stream, physical/reconciled origin, lifecycle
  and hook reinstallation status, and injected-event filtering. This is a scope
  summary, not an exhaustive upstream source diff or new test claim.
- The 2026-10-04 repair changed only `src/platform/windows/listener.rs` in the vendored tree:
  readiness timeout/disconnect requests stop **and joins the native hook thread**
  before the constructor returns its error. It no longer detaches an unfinished
  native thread. The explicit readiness-error branch already joined.
- SpeakMode invokes that constructor from its dedicated outer HotkeyService owner
  thread. The outer readiness wait remains bounded, returns the actual owner with
  its error, and supports retryable shutdown. The vendored constructor itself can
  wait beyond its readiness deadline for real cleanup; it must not be invoked on
  UI/engine-lock paths. This is a local behavioral change, not an upstream promise.
- SpeakMode 0.1.9 adds a narrow observed-listener standalone Alt/Win tap policy in
  `src/listener.rs`, exported through `src/lib.rs` and used by
  `src/platform/windows/listener.rs`. On a configured independent short tap,
  one native batch sends an inert menu mask followed by the corresponding
  modifier release. Only a fully accepted batch replaces the physical release.
  Chords, AltGr, long holds, disabled/capture policies and injected events do not
  use this replacement. Original license and upstream checkout remain unchanged.
  Behavioral tests and real physical-key acceptance are reported separately in
  the 0.1.9 experiment record; this source notice does not certify host input.

The first-party wrapper and lifecycle tests are in `platform/hotkeys.rs`; they
are separate from the upstream library license. Isolated thread-owner regression
passed and the Windows vendored source compiled with the GNU development toolchain.
No native hook was installed for these tests. Real native hook-fault injection and installation acceptance remain NOT_RUN in
this isolated hook receipt. Subsequent project MSVC compilation/test results are
separate evidence; compiler notice collection does not certify final CRT linkage.

## Tauri CLI 2.12.1 NSIS installer template

- Original source: `crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi` at
  `tauri-cli-v2.12.1`, commit `30da1fd6e17de6107ecc850c95dfb16b5729f2dd`.
  This release's bundler is 2.10.1, declared Apache-2.0 OR MIT. Both original
  license texts and SPDX metadata are retained under `supplements/`.
- Local modified source: `app/src-tauri/windows/installer-template.nsi`.
  The GUI version-change path now passes `/UPDATE` to the previous uninstaller.
  Upgrade/downgrade replacement therefore preserves profile data, the currently
  configured credential and autostart intent; it is not treated as an explicit
  destructive uninstall.
- First-party `windows/installer-hooks.nsh` uses the early native
  `--prepare-uninstall` handshake before installation/removal. Explicit data/Key
  deletion choices apply only to interactive uninstall (not update/passive/silent).
  The ordinary uninstall clears autostart entries; update keeps them. Data cleanup
  routes through the app's validated owned-root logic instead of stock NSIS
  recursive profile deletion.
- `/UPDATE` coordination requires the previously installed uninstaller/hooks to
  understand that flag and preservation contract. This custom path is pinned to
  CLI 2.12.1; future CLI/template updates and any older installed hook version
  require explicit compatibility review. Notices do not prove this migration path
  has been run successfully.
- Tauri resources are configured from `../../docs/third-party/nsis-resources/`
  to installed `third-party/`. Staging/hash verification is separate from actual
  bundle contents, signing, installation, upgrade and uninstall acceptance.
  Those installer flows remain NOT_RUN in this notice package.

Regenerate/stage after final dependency or notice source changes. The current
verification receipt covers the resulting material snapshot and original text
hashes, not final binary composition or legal review.

## macOS port (2026-10-23)

The local handy-keys changes in `src/listener.rs` and
`src/platform/macos/listener.rs` expose a monotonic event-tap reset epoch and
filter SpeakMode-tagged synthetic paste events. After a tap reset, the wrapper
discards pending gestures so a reconciled release cannot start another recording.
The MIT license and original attribution remain unchanged. Windows hook code
and its standalone Alt/Win masking policy were not changed by this port.

The Mac bundle includes this updated record separately as
`MACOS_LOCAL_SOURCE_MODIFICATIONS.md`; the Windows installer retains its existing
frozen resource snapshot. Neither record establishes physical-key acceptance.
