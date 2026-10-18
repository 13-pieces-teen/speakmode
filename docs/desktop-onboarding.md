# Desktop onboarding

The selected motion prototype becomes the real first-run surface. Existing
profiles can open it from Settings; upgrades do not pretend that users completed
training. Pausing preserves the current step and differs from completion.

Flow: welcome → cloud service / microphone setup → actual shortcut check → chat
dictation → document dictation → use in another app. Both lessons use the normal
capture, ASR, rewrite, source preservation and recovery pipeline. The native
engine overrides only that recording's delivery to Preview, disables nearby text
collection and chooses the lesson's built-in scene. Global settings stay intact.

Only actual output from an owned, finished practice session can satisfy a lesson.
Sample scripts are reading instructions, never seeded recognition/results.
Fallback text is labeled. Configured credentials do not count as a verified
connection; only actual practice can exercise the service. The final external-app
step restores normal delivery and does not claim to verify insertion receipts.

Use the original mist background and stable two-column stage. Page transitions
use short opacity/transform movement. Shortcut and recording feedback are
immediate; reduced motion removes transitions and decorative drift. The window
chrome, keyboard focus, error/retry paths and compact desktop sizes remain usable.

Validation will cover persisted first-run / legacy migration / pause and resume,
native route identity and delivery isolation, real command boundaries, the full
browser fixture flow and production build/package. Browser fixtures and synthetic
session tests do not establish real microphone or external-app acceptance.

Implementation and validation results: [0.2.24 report](experiments/desktop-onboarding-0.2.24-2026-10-18.md).

In 0.2.25, owned practices use the complete 20-second rewrite ceiling, rather than
the daily short-dictation 5-second budget. Fallback status, copy actions and the
continue button consistently identify the source. A saved fallback can be retried
without recording again through source/base-bound candidate generation and adoption;
unreviewed candidates are rejected and the original remains available. Continuing
with source requires acknowledging its exact output revision at the native boundary.
Long results scroll inside the result box. See the [0.2.25 report](experiments/onboarding-recorder-0.2.25-2026-10-18.md).
