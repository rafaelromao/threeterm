# ThreeTerm website

The landing page is `docs/index.html`, with CSS, JavaScript, and screenshots under
`docs/assets/`. It is plain static HTML: no Node dependency, bundler, or build step.
Links to project guides point to GitHub, where the Markdown is rendered.

## Deploy on GitHub Pages

1. In the repository's **Settings → Pages**, choose **GitHub Actions** as the source.
2. Merge the site files into `main`. `.github/workflows/pages.yml` uploads `docs/`
   and deploys whenever that directory or the workflow changes on `main`.
3. For the first deployment, you can also run **Actions → GitHub Pages → Run workflow**.

The expected URL is <https://rafaelromao.github.io/threeterm/>. The workflow exposes
the deployed URL in the `github-pages` environment. Repository settings still need
to be enabled by an administrator; the local files do not enable Pages themselves.

Alternatively, choose **Deploy from a branch**, `main`, `/docs`, in Pages settings.
The same static site works there. Use one deployment mode; the included workflow
is for the GitHub Actions mode. `docs/.nojekyll` makes branch deployment serve the
static files directly.

## Preview locally

From the repository root:

```sh
python3 -m http.server 8000 --directory docs --bind 127.0.0.1
```

Open <http://127.0.0.1:8000/>. Python is only needed for this preview, not for
deployment or running ThreeTerm. Barlow and Barlow Condensed are loaded from Google
Fonts; system fonts are used if that request is unavailable.

## Maintain accurate claims

Keep feature claims and installation snippets aligned with `README.md` and the
source. The automatic installer currently targets x86_64 Arch Linux, and the
qualified interactive environment is direct local Ghostty `1.3.1-arch2`.
Screenshots are shared with the README; their provenance is documented in
`assets/screenshots/README.md`. Update the `v0.1 / MVP` label when the user-facing
version changes.

`PRODUCT.md` and `DESIGN.md` at the repository root record the user-approved
audience and visual direction. The site provides keyboard focus, a skip link,
native expandable FAQ sections, responsive layouts, reduced-motion support, and
clipboard failure feedback. Commands remain selectable with JavaScript disabled.
