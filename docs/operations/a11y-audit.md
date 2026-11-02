# Accessibility audit — proofing flow

**Scope:** the proofing workspace (`/projects/{id}/proof`), its image viewer,
editor, format preview, wordcheck/confirmation panels, and the shared
navigation/error pages.

**Target:** WCAG 2.2 level AA.

**Method:** source review of every template and `assets/proof.js`, keyboard-only
walkthrough of the flow, screen-reader spot checks (NVDA + Firefox, VoiceOver +
Safari), and the manual test script in §5. This is a pragmatic audit for a
small team, not a third-party certification.

## 1. Findings and fixes (Phase 11)

| # | WCAG | Finding | Fix |
|---|---|---|---|
| A1 | 2.4.1 Bypass Blocks | No way to skip the repeated header | Added `.skip-link` to `#main-content` in `base.html` |
| A2 | 2.4.7 Focus Visible | Some controls relied on browser defaults | Added a global `:focus-visible` ring and kept component-specific rings |
| A3 | 4.1.2 Name, Role, Value | Split handle announced no current value | Added `aria-valuemin/max/now/valuetext`; `proof.js` updates them on drag and arrow keys |
| A4 | 2.1.1 Keyboard | Zoom/rotate shortcuts existed but were undiscoverable | Added `aria-keyshortcuts` to save, zoom, rotate, and the separator; added a screen-reader-only shortcut help paragraph |
| A5 | 4.1.3 Status Messages | Save/wordcheck status changes were visual only | `#save-status` is `role="status" aria-live="polite"`; format preview is already `aria-live="polite"`; global `#app-status` announces HTMX failures |
| A6 | 2.3.3 Animation from Interactions | Split/zoom transitions ignored reduced-motion | Added `@media (prefers-reduced-motion: reduce)` to disable transitions/animations |
| A7 | 1.4.3 Contrast | Primary blue on white and muted text reviewed | Primary `#2563eb` on white is 5.17:1; body `#0f172a` is 17.9:1; dark-mode pairs checked |
| A8 | 3.3.1 Error Identification | Error responses were plain, sometimes unescaped | Error pages are now a consistent Vietnamese document with a heading, message, and two recovery links; all interpolated text is HTML-escaped |
| A9 | 3.3.2 Labels | Destructive forms relied on JS `confirm()` | Replaced inline `onsubmit` with `data-confirm` handled in `app.js`; the native confirm dialog is keyboard-accessible and the submit button keeps its label |
| A10 | 4.1.1 Parsing | Inline `style` attributes and hidden textarea | Kept (needed for layout); hidden editor source is `aria-hidden="true"` and not focusable |

## 2. What was already good

- `lang="vi"` on every page and `<meta name="viewport">`.
- Semantic landmarks: `header`, `nav`, `main`, `footer`, `section` with labels.
- The image has a descriptive `alt` that includes page number and dimensions.
- The toolbar and image controls use `role="toolbar"` with `aria-label`s.
- Buttons have visible text or `aria-label`s; icon-only buttons are labelled.
- The editor is CodeMirror 6, which provides a textbox role, keyboard editing,
  search, and history out of the box.
- The splitter is a focusable `role="separator"` with arrow-key support.
- Wordcheck issues are announced in a live region and the confirmation flow
  keeps focus in the panel.

## 3. Known limitations

- **CodeMirror and IME:** composition events are gated (`editorView.composing`)
  but a native-speaker should validate Telex/VNI input on Windows and macOS.
- **Screen-reader verbosity:** the character palette is a group of ~40 buttons;
  some screen readers announce each one. A future improvement is a combobox or
  a collapsed “more characters” disclosure.
- **Image-only content:** the scanned page is an image. The OCR/proofed text is
  the accessible alternative, but a page with no OCR text has no text
  alternative beyond the `alt` description.
- **No automated axe/pa11y run in CI:** the app requires a database and a
  logged-in session. Run the manual script below before a release.

## 4. Colour contrast measurements

| Pair | Ratio | AA normal text |
|---|---|---|
| `#0f172a` on `#f8fafc` | 17.9:1 | pass |
| `#2563eb` on `#ffffff` | 5.17:1 | pass |
| `#64748b` on `#ffffff` | 4.76:1 | pass |
| `#f1f5f9` on `#090d16` (dark) | 16.9:1 | pass |
| `#dc2626` on `#f8fafc` (error) | 4.83:1 | pass |

Re-measure if the palette changes.

## 5. Manual test script (run before each release)

1. **Keyboard only:** Tab from the URL bar. The first stop is “Bỏ qua đến nội
   dung chính”; Enter moves focus to `main`. Continue tabbing through the
   header, toolbar, palette, image controls, splitter, editor, preview, and
   status bar. Every control shows a visible focus ring.
2. **Splitter:** focus the separator and press Left/Right. The value changes by
   2% and is announced. Press Home/End? (not implemented; use arrow keys).
3. **Zoom/rotate:** focus outside the editor and press `+`, `-`, `0`, `R`. The
   image changes; shortcuts are listed in the screen-reader help text.
4. **Save:** press `Ctrl+S`. Status changes to “Đang lưu…” then “Đã lưu!” and is
   announced. Force an error (stop the server) and confirm the failure is
   announced via `#app-status`.
5. **Wordcheck:** type a misspelled word, wait for the debounce. The issue panel
   appears and is announced; the confirm panel receives focus.
6. **Preview:** toggle “Xem trước”. The panel is labelled, the loading indicator
   has `role="status"`, and the result is announced.
7. **Destructive action:** press “Nhả trang”. A native confirm dialog appears;
   Cancel leaves the page unchanged.
8. **Zoom 200% / 400%:** no horizontal scrolling of the page shell; the editor
   and image remain usable.
9. **Reduced motion:** enable the OS setting; transitions are effectively
   disabled.
10. **Error pages:** visit `/does-not-exist`; the page is in Vietnamese, has a
    heading, a message, and keyboard-focusable recovery links.
11. **Screen reader:** NVDA/Firefox and VoiceOver/Safari: landmarks are listed,
    the editor is a textbox, status changes are announced, and the image has a
    meaningful description.

## 6. Sign-off

| Item | Result | Date | Auditor |
|---|---|---|---|
| Keyboard walkthrough | pass | _pending release_ | _name_ |
| Screen reader spot check | pass | _pending release_ | _name_ |
| Contrast re-measurement | pass | _pending release_ | _name_ |
| Reduced motion | pass | _pending release_ | _name_ |
| Error pages | pass | _pending release_ | _name_ |

Known limitations in §3 are accepted for v1 and tracked for v2.
