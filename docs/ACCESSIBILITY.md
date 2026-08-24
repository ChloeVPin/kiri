# Accessibility baseline

Kiri's shipped starter and demo frontends provide an accessible baseline for
the example experience. This is an engineering target, not a legal
conformance claim.

## Implemented behavior

- Both examples declare `lang="en"`, use a single `main` landmark, and keep a
  logical heading hierarchy.
- A keyboard-visible skip link moves focus to the main content.
- Native buttons and the native textarea remain the interactive controls.
- Focus-visible outlines remain present instead of being replaced by color
  changes alone.
- Connection, boot, and action feedback uses a polite status region or log.
- The demo notepad has a programmatic label.
- Reduced-motion users receive a reduced transition/scroll behavior policy.
- Decorative Kiri artwork uses an empty alternative so it does not interrupt
  the reading order.

## Review evidence

Source inspection covers `examples/starter/index.html` and
`examples/demo/index.html`. The keyboard path is: skip link, page headings,
native controls, and status/log feedback. The examples use no custom
keyboard widget, modal, drag interaction, or time-limited task.

Automated source checks and runtime smoke tests verify that the examples are
served and that the native bridge reaches the required startup markers. A
manual keyboard pass and screen-reader pass should still be performed on each
supported OS before a release is advertised as accessible. No disabled-user
evaluation or platform screen-reader test is claimed by this document.

## Revisit triggers

Repeat this review when adding custom dialogs, menus, focus-managed overlays,
keyboard shortcuts, animation, localization, authentication, or a new
starter interaction. Test zoom/reflow, high contrast, reduced motion, slow
bridge failure, and error feedback for every new journey.
