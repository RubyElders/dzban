---
name: dzban-ui-testing
description: Exercise and visually verify Dzban GTK/libadwaita behavior under a controlled display. Use for widget state, keyboard interaction, layout, animation, styling, accessibility, or other changes whose correctness depends on rendered UI behavior.
---

# Dzban UI Testing

Do not claim UI behavior works from compilation or property assertions alone. Use the narrowest useful layers:

1. Test pure view-state formatting with ordinary Rust tests.
2. Test GTK widget state and signals with `#[gtk4::test]`; GTK tests are serialized on GTK's test thread.
3. Put keyboard and chord interpretation in a pure state machine and test it without a display. Use GTK tests for focus, accessible properties, and widget reactions.
4. When an external black-box interaction test is useful, prefer an AT-SPI driver such as Dogtail and stable accessible names. Do not make screen coordinates the behavioral assertion.
5. For theme painting, animation, popovers, or layout stability, run the real application in a virtual display and inspect captured frames. Coordinate replay is only an aid for reaching visual states.
6. When the UI depends on query behavior, combine the UI run with the disposable database environment from `skills/dzban-testing`.

Read [references/dogtail-integration.md](references/dogtail-integration.md) before adding or changing a screen-level workflow. Run the maintained suite with `scripts/run-dogtail-integration.sh`.

Read [references/visual-testing.md](references/visual-testing.md) before a pixel-level rendered test. The reusable visual session launcher is `scripts/visual-session.sh`.

Capture before, transient, completed, and reset states when testing animation. Compare stable geometry and relevant pixel regions, then inspect the images. A screenshot existing is not by itself a passing assertion.
