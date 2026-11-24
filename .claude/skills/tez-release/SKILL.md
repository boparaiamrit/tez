---
name: tez-release
description: Use when releasing or tagging a tez version ("release v0.2.8", "cut a release", "ship it", "tag it"), when asked whether a release run is done, or when changing the release workflow, the Windows installer (packaging/windows/tez.iss) or the .deb and .rpm packaging.
---

# Releasing tez

Pushing a tag `vX.Y.Z` runs `.github/workflows/release.yml`. Windows, macOS (Apple silicon) and Linux
each run the unit tests, build, smoke-test, package, install-test their packages, and publish to one
GitHub release. It takes about 10 to 20 minutes.

## Before the tag

1. On `master`, clean and level with `origin`. `git log vPREV..HEAD --oneline` must not be empty.
2. Bump `version` in **both** `Cargo.toml` and `app/Cargo.toml`. Then run
   `cargo build --release --workspace -j 4`, which refreshes `Cargo.lock` (only the tez and tez-app
   entries change; never edit the lock by hand), and `cargo test -p tez -j 4`.
   `target/release/tez version` must print the new version.
3. Privacy scan, because the repo is public. Search the tree and the new commit messages for the
   user's real project names (every name in `tez sites` other than demo names), their username and
   their home path:
   `git grep -n -i -E "<names>"` and `git log vPREV..HEAD --format=%B | grep -i -E "<names>"`.
   Both must find nothing. Never write those names into the repo, the release notes or this file.
4. If the workflow, `packaging/` or `scripts/` changed, do a trial run first:
   `gh workflow run release.yml --ref master`. It builds, tests and install-tests every package and
   keeps them as run artifacts, but publishes nothing (Publish runs only for tags).
5. Commit as `X.Y.Z: <what changed>` and push `master`. Wait for CI on that commit to pass:
   `gh run list --workflow CI --branch master --limit 1`, then `gh run watch <id> --exit-status`.

## Tag and watch

6. `git tag vX.Y.Z`, then `git push origin vX.Y.Z`.
7. `gh run list --workflow Release --limit 1`, then `gh run watch <id> --exit-status`, in the background.
   While it runs, the answer to "is it done?" is "not yet": say which jobs are still going.

## Before you say "released"

8. `gh release view vX.Y.Z --json assets,isDraft,isPrerelease,url`. It must not be a draft, and it
   must have all seven assets:
   - `tez-vX.Y.Z-windows-x64.zip` and `tez-vX.Y.Z-windows-x64-setup.exe`
   - `tez-vX.Y.Z-macos-arm64.dmg` and `tez-vX.Y.Z-macos-arm64-cli.tar.gz`
   - `tez-vX.Y.Z-linux-x64.tar.gz`, `.deb` and `.rpm`
9. Each one downloads without signing in:
   `curl -sIL -o /dev/null -w "%{http_code}\n" <browser_download_url>` prints 200.
10. The workflow writes a commit list as the notes. Replace it with what changed for people using tez:
    `gh release edit vX.Y.Z --notes-file <file>`. The privacy rule applies here too.

## When a job fails

The three jobs publish separately, so one failure can leave a partial release. Report the failing step
(`gh run view <id> --log-failed`) and propose a fix. Never delete or move a pushed tag, or delete a
release, without the user's go-ahead. The fix normally ships as the next patch version.

## Known limits to state, not rediscover

- The Linux packages need glibc 2.39 or newer (Ubuntu 24.04, Debian 13, Fedora 40, RHEL 10).
- The Windows installer is not code-signed, so SmartScreen warns. The macOS app is ad-hoc signed, not
  notarised.
- macOS builds only in this workflow, so a tag or a trial run is the first macOS signal.
- CI installs the installers silently, so their wizard pages are never tested.

## The final message

The release URL, what changed for users, the result on each of the three systems, the seven assets
checked, and what was not tested. The tez on this machine still runs the old version until the user
asks for a deploy (see the tez-development skill).
