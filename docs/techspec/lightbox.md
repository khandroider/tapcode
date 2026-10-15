# Clickable screenshots on the landing

## Goal

Let visitors click a screenshot and see it big. Text in the cards is too small to read.

## Context

The landing is `site/index.html` + `site/style.css`, no JS. See [docs/modules/site.md](../modules/site.md).
Six screenshots: one big in the hero, five in the Screens grid. All are plain `<img>` inside `<figure>`.

## In scope

- Every screenshot opens in a lightbox on click.
- Close by click on the dark background, the close button, or Esc.
- Works without JS: the link opens the image file.
- Inline JS only, no libraries.

## Out of scope

- Arrows or swipe between images.
- Zoom inside the lightbox.
- Any change to the images themselves.

## Requirements

- Keyboard: Tab reaches each screenshot link, Enter opens it, Esc closes.
- Screen readers: the link has the image alt text; the dialog has a label.
- Mobile: the image fits the screen, no horizontal scroll at 375px.
- No console errors. Page size grows by under 2 KB.

## Assumptions

- Native `<dialog>` is fine (all current browsers support it).
- The lightbox shows the same 1400×900 files that the cards use. No bigger versions.

## Plan

### 1. Wrap images in links

- **Do** — in `site/index.html` wrap each `<img>` of `.shot` in `<a class="zoom" href="img/<file>">`.
- **DoR** — list of the six images is known (it is).
- **DoD** — with JS off, click opens the image file in the tab. Tab key stops on each link.

### 2. Add the dialog and script

- **Do** — add `<dialog id="lightbox" aria-label="Screenshot"><button>Close</button><img></dialog>` before `</body>` and a `<script>`: click on `a.zoom` → prevent default, copy `href` and `alt` into the dialog image, `showModal()`. Click on the dialog background or the close button → `close()`. Esc works by itself.
- **DoR** — item 1 done.
- **DoD** — click opens the right image with the right alt. Esc, background and button close it. No console errors.

### 3. Style it

- **Do** — in `site/style.css`: `.zoom { cursor: zoom-in }`, dialog with no border and transparent background, image `max-width: 95vw; max-height: 90vh`, `::backdrop` dark at 85%, close button in the top right corner.
- **DoR** — item 2 done.
- **DoD** — looks right at 1440px and 375px. Image never overflows the screen.

### 4. Check locally

- **Do** — serve `site/`, open in the browser, test desktop and mobile widths.
- **DoR** — items 1 to 3 done.
- **DoD** — all DoD above pass. Screenshot of the open lightbox saved in the PR description.

### 5. Ship

- **Do** — branch `site/lightbox`, PR to `master`, merge, Pages deploy.
- **DoR** — item 4 done.
- **DoD** — live site opens a lightbox on click.

## Acceptance

1. Open https://khandroider.github.io/tapcode/ and click any screenshot. It opens big.
2. Press Esc. It closes. Click the background. It closes.
3. Try on a phone. Same.

## Open questions

None.
