# Personal local-path branch

Repository: https://github.com/Ly4hm/herdr

Branch: `feature/local-path-click`

Upstream: https://github.com/herdrdev/herdr

This branch adds Ctrl+click on local paths to open directories or reveal files in
the system file manager, with Ctrl-hover underlines. Relative paths use the local
pane's foreground working directory, falling back to its shell working directory.
Remote endpoint paths are not opened on the client machine. WSL uses Windows
Explorer and requests foreground activation of the matching window.

Plain-text paths are recognized within the clicked physical terminal row. The
v0.9.0 stable pane-surface protocol does not include soft-wrap metadata, so this
branch does not guess whether adjacent rows form one path. A plain path split
across rows may therefore fail to open. OSC 8 `file://` hyperlinks retain their
full destination and support labels spanning multiple rows. Existing HTTP/HTTPS
link handling is preserved.

The original implementation is preserved in commit `877519c`, based on v0.8.2.
The current branch adapts it to the v0.9.0 client shell.

## Updating from upstream

The **Sync stable upstream for local path fork** GitHub Actions workflow checks
for the latest published upstream stable release daily around 03:23 UTC
(11:23 Asia/Shanghai; GitHub may delay scheduled runs). It can also be run from
Actions → Run workflow, optionally specifying a stable tag such as `v0.9.0`.
Manual runs always validate even when no new release exists.

It merges the release into this branch, checks formatting and Clippy, runs the
Rust tests and architecture checks, and builds a Linux executable. Only successful
validation permits a normal, non-force push. Merge conflicts or failing checks
leave the remote branch unchanged and appear as a failed Actions run. Resolving
such failures requires a reviewed code change; no automatic conflict choices are
made. Upstream preview releases and unreleased master commits are excluded.

Builds are available as `herdr-local-path-linux-x86_64` artifacts for 14 days.
They are personal Linux builds, not official releases. This workflow does not
install software on any workstation or change a running Herdr session.

To merge manually in a clean checkout:

```sh
git switch feature/local-path-click
git pull --ff-only origin feature/local-path-click
python3 scripts/fork_sync.py --tag v0.9.0 --validate-always
# Run the checks from .github/workflows/fork-sync.yml before pushing.
git -c push.followTags=false push origin HEAD:feature/local-path-click
```

Only the personal synchronization workflow is enabled in this fork. Do not push
upstream release tags or enable inherited deployment/release workflows. The fork
uses the GitHub Actions token with repository contents permission; no upstream
write permission or personal access token is required by the sync workflow.

Official `herdr update` installs the upstream build and would replace this feature.
Use this branch's build when the extension is needed.
