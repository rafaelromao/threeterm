# Product Name Evaluation

Status: research note, not trademark or domain counsel.

Checked: 2026-07-30.

## Scope and Limits

This evaluates public GitHub repository-name search and semantic fit. It does not establish trademark clearance, exact package-name availability, domain availability, social-handle availability, or permission to use a mark. Those need a release-readiness check after a name is selected.

## Evidence

GitHub search results:

| Candidate | Result | Product fit | Assessment |
| --- | --- | --- | --- |
| ForgeTTY | Three fuzzy name-search results, none exact. | Forge evokes making; `TTY` makes terminal explicit. Could imply build tooling or a terminal itself rather than CAD. | Usable runner-up. |
| BrepTTY | No repository-name search results. | Technically precise, but B-rep jargon makes the product harder to discover for users who know printing but not CAD internals. | Technically clear but not recommended. |
| Termesh | Five matches. Two exact-name projects are terminal 3D mesh renderers. | Memorable, but `mesh` misstates exact parametric/B-rep model truth and collides in the same product neighborhood. | Reject. |
| Shellform | Five fuzzy matches, including `ShellFormFinding`, a 3D compression-form design tool. | Connects shell and shape but is generic and overlaps adjacent geometry/design concepts. | Not preferred. |
| Partline | Four matches, including exact-name repositories. | Parts are relevant; terminal and parametric modeling are not apparent. | Not preferred. |
| FormTTY | No repository-name search results. | Accessible and terminal-specific, but does not convey solids or 3D printing as clearly as ThreeTerm. | Usable runner-up. |

## Sources

- GitHub repository search, `ForgeTTY in:name`, accessed 2026-07-30: <https://api.github.com/search/repositories?q=ForgeTTY%20in%3Aname&per_page=10>
- GitHub repository search, `BrepTTY in:name`, accessed 2026-07-30: <https://api.github.com/search/repositories?q=BrepTTY%20in%3Aname&per_page=10>
- GitHub repository search, `Termesh in:name`, accessed 2026-07-30: <https://api.github.com/search/repositories?q=Termesh%20in%3Aname&per_page=10>
- GitHub repository search, `Shellform in:name`, accessed 2026-07-30: <https://api.github.com/search/repositories?q=Shellform%20in%3Aname&per_page=10>
- GitHub repository search, `Partline in:name`, accessed 2026-07-30: <https://api.github.com/search/repositories?q=Partline%20in%3Aname&per_page=10>
- GitHub repository search, `FormTTY in:name`, accessed 2026-07-30: <https://api.github.com/search/repositories?q=FormTTY%20in%3Aname&per_page=10>

## Selected Name

The product owner selected **ThreeTerm**. It evokes 3D and terminal use without TTY jargon. This selection was made after this initial candidate research, so exact namespace and trademark status remain unverified.

No package, documentation, domain, or branding namespace should be locked beyond the repository rename before targeted release-namespace review.

## Open Questions

- Are exact names available or acceptable in intended package registries, domains, social platforms, and Linux distribution namespaces?
- Does a trademark search in intended markets expose a conflicting mark?
- Does ThreeTerm have acceptable risk across intended package registries, Linux distribution namespaces, domains, social handles, and relevant trademark databases?
