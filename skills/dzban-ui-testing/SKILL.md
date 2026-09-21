---
name: dzban-ui-testing
description: Exercise and visually verify Dzban GTK/libadwaita behavior under a controlled display. Use for widget state, keyboard interaction, layout, animation, styling, accessibility, or other changes whose correctness depends on rendered UI behavior.
---

# Dzban UI Testing

Do not claim UI behavior works from compilation or property assertions alone. Use the narrowest useful layers:

1. Test pure view-state formatting with ordinary Rust tests.
2. Test GTK widget state and signals with `#[gtk4::test]`; GTK tests are serialized on GTK's test thread.
3. For theme painting, focus, keyboard routing, animation, popovers, or layout stability, run the real application in a virtual display and inspect captured frames.
4. When the UI depends on query behavior, combine the visual run with the disposable PostgreSQL environment from `skills/dzban-testing`.

Read [references/visual-testing.md](references/visual-testing.md) before a rendered test. The reusable session launcher is `scripts/visual-session.sh`.

Capture before, transient, completed, and reset states when testing animation. Compare stable geometry and relevant pixel regions, then inspect the images. A screenshot existing is not by itself a passing assertion.
