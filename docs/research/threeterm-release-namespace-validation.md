# ThreeTerm Release Namespace Validation

Status: preliminary technical and namespace screening, not legal clearance.

Checked: 2026-07-30.

## Conclusion

ThreeTerm is usable for internal development, but it is **not cleared for irreversible public branding or release**.

Most queried package, Linux, domain, and developer-account endpoints returned no exact record. However, GitHub has an earlier exact-name project, [`amalczanek/threeterm`](https://github.com/amalczanek/threeterm), described as "An on-screen terminal usable in your three.js applications." It is inactive since 2018 and has no stars or forks, but it is software in a related terminal category. That creates real searchability and source-confusion risk.

Official trademark systems also require interactive, similarity, goods/services, and common-law analysis that this automated screening cannot complete. USPTO explicitly says comprehensive clearance includes similar marks, related goods/services, federal and state records, domains, international databases, and internet/common-law use.

## Earlier Project Activity Check

Checked: 2026-07-30.

GitHub metadata for `amalczanek/threeterm` shows that it is not actively maintained:

- last commit and last push: 2018-01-16;
- no releases, tags, or issues;
- zero stars and forks;
- repository is neither archived nor disabled.

Its maintained status is not a legal-clearance conclusion. It only confirms that the documented exact-name terminal-software collision is dormant at the hosted repository level.

## Package Registries

Exact public API endpoints returned HTTP 404 for `threeterm` on:

- npm
- PyPI
- crates.io
- RubyGems
- NuGet
- Maven Central path
- Packagist vendor/package path

A 404 means no public record existed at that exact endpoint when checked. It does not guarantee that a registry will permit registration or that a reserved, private, scoped, or similar name is safe.

## Linux and Application Distribution

No exact result was returned by:

- Arch Linux official package search
- Arch User Repository search
- Fedora package search
- Homebrew formula API
- Homebrew cask API
- Flathub appstream API

Debian's public search endpoint presented an automated-access challenge, so Debian and Ubuntu exact package availability remains unverified. Snap Store lookup also remains unverified because its API requires store-specific request metadata.

## Source Hosting and Developer Handles

- GitHub repository search found three name matches. Material conflict: `amalczanek/threeterm`, an earlier terminal-software project. The other relevant exact result is this repository.
- GitHub returned no exact user or organization named `threeterm`.
- GitLab returned no exact username `threeterm`.
- Codeberg returned no exact user `threeterm`.
- Social-platform handles were not comprehensively checked. Handle availability changes quickly and should be reserved only after name-risk acceptance.

## Domains

Registry RDAP endpoints returned HTTP 404 for:

- `threeterm.com`
- `threeterm.net`
- `threeterm.org`
- `threeterm.dev`
- `threeterm.io`
- `threeterm.app`

This means those RDAP services returned no registration object when checked. It is not a registrar availability promise; names may be premium, reserved, blocked, or registered between check and purchase.

## Trademark and Common-Law Limits

No trademark clearance conclusion is possible from this automated pass.

USPTO states that likelihood of confusion depends on similarity in sound, appearance, meaning, or commercial impression plus related goods/services. It also recommends a comprehensive search covering federal records, state records, domains, Madrid/WIPO, EUIPO/TMview, internet results, and common-law use. USPTO, WIPO, and TMview search applications require interactive workflows or anti-automation checks in this environment.

The earlier exact-name terminal-software repository is itself relevant common-law/searchability evidence even if no registered mark exists.

## Recommendation

Do not reserve broad public branding or publish packages yet. Ask the product owner to choose among:

1. retain ThreeTerm and explicitly accept the exact terminal-software collision;
2. contact the earlier project owner and investigate coexistence or transfer;
3. choose a more unique name before public release.

If ThreeTerm is retained, perform a professional or owner-driven comprehensive trademark search in intended markets and reserve critical package/domain/handle namespaces promptly.

## `3Term` Alternative

`3Term` is not a lower-risk replacement:

- GitHub name search returned 116 matches. Exact `codeDirtyToMe/3Term` is a shell script for opening three terminals; other exact `3term` repositories also exist.
- `3term.com` is registered through 2027 according to Verisign RDAP.
- npm, PyPI, and crates.io returned no exact package record, and `.dev` RDAP returned no registration object.
- A leading numeral is valid for a shell command and many package registries, but not for identifiers in many programming languages. Product, executable, module, namespace, and configuration names would need inconsistent spellings.
- Search results mix software with common "third term" and "three-term" phrases, reducing discoverability.

Recommendation: do not rename ThreeTerm to `3Term`.

## Owner Decision

The product owner accepted the documented exact-name terminal-software collision and retained ThreeTerm. This resolves product-name selection, but it does not replace comprehensive trademark/common-law clearance before public release.

## Sources

- Existing exact-name repository: <https://api.github.com/repos/amalczanek/threeterm>
- GitHub name search: <https://api.github.com/search/repositories?q=threeterm%20in%3Aname&per_page=20>
- npm: <https://registry.npmjs.org/threeterm>
- PyPI: <https://pypi.org/pypi/threeterm/json>
- crates.io: <https://crates.io/api/v1/crates/threeterm>
- RubyGems: <https://rubygems.org/api/v1/gems/threeterm.json>
- NuGet: <https://api.nuget.org/v3-flatcontainer/threeterm/index.json>
- Maven Central: <https://repo1.maven.org/maven2/threeterm/>
- Packagist: <https://packagist.org/packages/threeterm/threeterm.json>
- Arch Linux: <https://archlinux.org/packages/search/json/?name=threeterm>
- AUR: <https://aur.archlinux.org/rpc/v5/search/threeterm?by=name>
- Fedora: <https://packages.fedoraproject.org/search?query=threeterm>
- Homebrew formula: <https://formulae.brew.sh/api/formula/threeterm.json>
- Homebrew cask: <https://formulae.brew.sh/api/cask/threeterm.json>
- Flathub: <https://flathub.org/api/v2/appstream/threeterm>
- GitHub user: <https://api.github.com/users/threeterm>
- GitHub organization: <https://api.github.com/orgs/threeterm>
- GitLab user: <https://gitlab.com/api/v4/users?username=threeterm>
- Codeberg user: <https://codeberg.org/api/v1/users/threeterm>
- `.com` RDAP: <https://rdap.verisign.com/com/v1/domain/THREETERM.COM>
- `.net` RDAP: <https://rdap.verisign.com/net/v1/domain/THREETERM.NET>
- `.org` RDAP: <https://rdap.publicinterestregistry.org/rdap/domain/threeterm.org>
- `.dev` RDAP: <https://pubapi.registry.google/rdap/domain/threeterm.dev>
- `.io` RDAP: <https://rdap.org/domain/threeterm.io>
- `.app` RDAP: <https://pubapi.registry.google/rdap/domain/threeterm.app>
- USPTO comprehensive clearance guidance: <https://www.uspto.gov/trademarks/search/comprehensive-clearance-search-similar-trademarks>
- USPTO likelihood-of-confusion guidance: <https://www.uspto.gov/trademarks/search/likelihood-confusion>
- USPTO trademark search: <https://tmsearch.uspto.gov/>
- WIPO Global Brand Database: <https://branddb.wipo.int/>
- TMview: <https://www.tmdn.org/tmview/>
- GitHub `3term` name search: <https://api.github.com/search/repositories?q=3term%20in%3Aname&per_page=20>
- Existing exact `3Term` terminal script: <https://github.com/codeDirtyToMe/3Term>
- `3term.com` RDAP: <https://rdap.verisign.com/com/v1/domain/3TERM.COM>
- npm `3term`: <https://registry.npmjs.org/3term>
- PyPI `3term`: <https://pypi.org/pypi/3term/json>
- crates.io `3term`: <https://crates.io/api/v1/crates/3term>
- `.dev` `3term` RDAP: <https://pubapi.registry.google/rdap/domain/3term.dev>
