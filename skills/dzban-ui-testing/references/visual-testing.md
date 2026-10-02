# GTK visual and interaction testing

## Native GTK tests

Use `#[gtk4::test]` for tests that construct or mutate widgets. This macro initializes GTK on one serialized test thread. Use `gtk4::test_widget_wait_for_draw()` when a realized widget must finish drawing. GTK also exposes `test_accessible_has_property`, `test_accessible_has_relation`, `test_accessible_has_role`, and `test_accessible_has_state` for accessibility contracts.

Tests requiring a display can be marked ignored with a precise reason and run under the virtual-display workflow. Keep logic tests non-ignored.

## Interaction layers

Test shortcut and chord interpretation as a pure Rust state machine. Test the resulting widget state, signals, focus, and accessibility contracts with `#[gtk4::test]`. This gives deterministic behavioral coverage without a window manager or synthetic input.

For a black-box test of the running application, use the maintained Dogtail routine in [dogtail-integration.md](dogtail-integration.md). Target accessible roles, labels, and descriptions rather than screen coordinates. GTK does not provide a Playwright-style application driver; its native test API is intended for in-process widget tests.

Use `xdotool` only to place the real application into a state for visual inspection when a semantic driver is unavailable. A coordinate click or synthetic key event is not sufficient behavioral evidence by itself.

## Virtual display

The host may already export `WAYLAND_DISPLAY`; if it is left in place, the application can open on the user's real desktop instead of Xvfb. Force X11 and use a disposable D-Bus session:

```bash
skills/dzban-ui-testing/scripts/visual-session.sh
```

Inside that shell, coordinate replay may be used to reach a visual state:

```bash
/absolute/path/to/target/debug/dzban > /tmp/dzban-ui.log 2>&1 &
win=$(xdotool search --onlyvisible --name 'Dzban' | head -n1)
xdotool mousemove --window "$win" X Y click 1
xdotool keydown Control_L
import -window "$win" /tmp/loading.png
xdotool keyup Control_L
import -window "$win" /tmp/reset.png
```

Use a temporary working directory containing only a test `dzban.toml`; never launch a visual test from the repository root when its local file contains user credentials. Prefer the Compose database fixtures.

Use `view_image` to inspect frames. ImageMagick can assert quantitative differences or unchanged geometry:

```bash
magick identify /tmp/loading.png /tmp/reset.png
magick compare -metric RMSE /tmp/loading.png /tmp/reset.png null:
magick /tmp/loading.png -crop WIDTHxHEIGHT+X+Y info:
```

Crop comparisons to the widget under test so cursor movement, timers, toasts, or database latency elsewhere do not create false positives. For an animation, capture at least two loading frames and verify the painted region moves; capture the terminal frame and verify the transient paint is absent. Inspect the final images even when pixel assertions pass.

Stop the app, exit the visual session, remove temporary files, and tear down Compose services after testing.
