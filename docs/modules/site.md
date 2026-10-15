# site

**What it does** — The landing page at https://khandroider.github.io/tapcode/. One HTML file, one CSS file, no JS.

**Key files**
- `site/index.html` — hero, features, screenshots, build section, footer.
- `site/style.css` — Palenight colors, JetBrains Mono, responsive grid.
- `site/img/*` — screenshots from the sample project, 1400×900.
- `.github/workflows/pages.yml` — deploys `site/` to GitHub Pages on push to `master`.

**Entry points** — a push to `master` that touches `site/`.

**Talks to** — GitHub Pages. `dev/demo.sh` + `dev/screenshots.sh` make the images.

**Watch out** — screenshots are plain `<img>`, not links. Images are shown at card size, so text in them is small.
