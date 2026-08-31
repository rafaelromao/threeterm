# libslvs Distribution Contract

ThreeTerm distributes the selected SolveSpace `libslvs` worker under the
GNU General Public License version 3, SPDX identifier `GPL-3.0-only`. No
commercial libslvs license is claimed by this repository.

Every binary worker artifact must include:

- `LICENSES/GPL-3.0-only.txt`, the complete license text;
- `NOTICE`, identifying worker `slvs`, worker schema
  `threeterm.workers.slvs/1`, and the pinned source revision;
- `SOURCE-OFFER.txt`, the written corresponding-source offer; and
- `licenses/libslvs.json`, the machine-readable policy used to verify the
  package and artifact metadata.

The corresponding source is SolveSpace commit
`27b6a080c8b669421bd4d444650c3b8eddec5687` at:

`https://github.com/solvespace/solvespace/tree/27b6a080c8b669421bd4d444650c3b8eddec5687`

The staged artifact manifest records the SHA-256 digest of each required file
and the worker executable. Relocation is supported because all manifest paths
are relative to the artifact root. Verify a staged artifact before any public
release with:

```sh
.github/scripts/release.sh verify-artifact \
  target/libslvs-artifact/manifest.json target/libslvs-artifact
```

Canonical CI builds and executes the pinned worker, writes the native-worker
evidence manifest, stages this artifact bundle, and refuses inconsistent or
incomplete licensing metadata.
