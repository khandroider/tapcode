# tapcode landing page

## Goal

A one-page site that shows what tapcode is and sends people to GitHub.
We want stars, contributors and first users.

## Context

tapcode is a Rust IDE for PHP / JS / TS built on egui. See [docs/modules](../modules/README.md).
There is no site and no public GitHub repo yet. Screenshots exist in `docs/*.png`.
Themes live in `assets/themes/*.toml`, font in `assets/fonts/`.

## In scope

- `site/index.html` and `site/style.css`. No build step, no JS framework.
- Dark look in Palenight colors, JetBrains Mono font.
- English text. Short.
- Sections: hero, features, screenshots, build from source, footer.
- GitHub Actions workflow that deploys `site/` to GitHub Pages.

## Out of scope

- Downloads of a built `.app` (no releases yet).
- Email signup, analytics, blog, docs pages.
- Russian version.

## Requirements

- Works on phone and desktop. No horizontal scroll at 375px.
- Loads fast: one HTML, one CSS, images under 500 KB each.
- No external scripts. Font copied into `site/fonts/`.
- All links work. The GitHub link is one constant at the top of `index.html`.
- Page looks fine with images missing (alt text, fixed box size).

## Assumptions

- GitHub URL will be `https://github.com/khandroider/tapcode` (gh is logged in as khandroider). Change in one place if wrong.
- Pages URL will be `https://khandroider.github.io/tapcode/`.
- Screenshots are reused from `docs/*.png`, resized if too big.
- Feature list is taken from the README table, cut to 8 items.

## Plan

### 1. Create the GitHub repo

- **Do** — create public repo `tapcode`, push `main`. Update `repository` in `Cargo.toml` if the URL differs.
- **DoR** — user picks the owner (khandroider by default). `gh` is logged in.
- **DoD** — `git push` works. Repo page opens in a browser.

### 2. Copy assets into `site/`

- **Do** — `site/fonts/JetBrainsMono-Regular.ttf` and `-Bold.ttf`, `site/img/*.png` from `docs/`, `site/favicon.png` from `assets/icon/tapcode-256.png`.
- **DoR** — item 1 done or in progress; screenshots in `docs/` are current.
- **DoD** — every image under 500 KB. `ls site/img` shows 6 files. Font OFL.txt copied too.

### 3. Write `site/index.html`

- **Do** — hero (name, one line, "View on GitHub" button), features grid (8 cards), screenshots (one big + a row of small), "Build from source" with `cargo run --release -- ~/project`, footer (MIT, GitHub link).
- **DoR** — item 2 done. GitHub URL known.
- **DoD** — valid HTML (no errors in browser console). Every link opens the right place. Text has no typos.

### 4. Write `site/style.css`

- **Do** — Palenight colors from `assets/themes/palenight.toml`, JetBrains Mono via `@font-face`, grid that folds to one column on phone.
- **DoR** — item 3 done.
- **DoD** — no horizontal scroll at 375px and 1440px. Text contrast passes 4.5:1. Looks right in Chrome and Safari.

### 5. Deploy to GitHub Pages

- **Do** — `.github/workflows/pages.yml`: on push to `main`, upload `site/` and deploy with `actions/deploy-pages`. Turn on Pages in repo settings (source: GitHub Actions).
- **DoR** — items 1 and 4 done.
- **DoD** — workflow is green. `https://khandroider.github.io/tapcode/` shows the page.

### 6. Link from README

- **Do** — add the site URL to the top of `README.md`.
- **DoR** — item 5 done.
- **DoD** — link in README opens the live site.

## Acceptance

1. Open the Pages URL on a laptop and on a phone. The page looks right.
2. Click "View on GitHub". The repo opens.
3. Copy the build command, run it. It works.
4. Lighthouse performance score 90 or higher.

## Open questions

- Repo owner: khandroider or another account?
