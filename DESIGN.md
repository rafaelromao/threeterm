# ThreeTerm website design

## Scene and direction

A hobbyist reads the site beside a sunlit workbench, comparing dimensions on a
small printed bracket before returning to their terminal. Use a light surface
and the ochre/graphite contrast of workshop measuring equipment. The reference
is a practical bench-tool label, not a software dashboard.

## Color strategy

Committed: an ochre hero and installation section carry the page's identity.
Warm off-white separates the workflow; graphite anchors commands and screenshots.
All website colors are OKLCH tokens in `docs/assets/site.css`. App screenshots
retain the app's actual palette.

## Typography

Brand voice: warm, practical, mechanical. Barlow Condensed headings evoke stamped
tool packaging; Barlow body text keeps setup instructions readable. Both are
available from the Google Fonts catalog. System fallbacks remain usable offline.
Monospace is reserved for real commands, JSON, and key labels.

## Layout and components

Asymmetric hero: large left-aligned headline and a dominant actual screenshot.
Ruled feature lists rather than a repeated card grid. A sequential three-step
workflow, expandable screenshots, copyable install commands, and compact FAQ.
Use square corners, clear full borders, large clickable targets, and visible
focus rings. Body copy stays within 70 characters per line.

## Motion and responsiveness

No decorative entrance animation. Small color transitions only; respect reduced
motion. Two-column regions collapse into a single column on narrow screens.
Command blocks scroll locally; the page itself must never overflow horizontally.
