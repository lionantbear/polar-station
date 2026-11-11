# Updating the public repository safely

Keeping this GitHub repository does not require keeping obsolete files in its
current tree. A reviewed synchronization commit can delete files while retaining
prior commits. This tool does **not** erase files from Git history, and is not a
secret-removal/history-rewrite tool.

## One-way ownership

PolarInfra is the only editable source of product code. Polar Station source
files are generated output: do not maintain a second codebase or copy changes
back from the public repository. The saved previous snapshot is verification
state, not another working source tree. Each synchronization makes managed paths
match the new export, including deletions. Repository-only CI/configuration is
the explicit exception listed below. Unexpected remote edits are an error, not
an invitation to merge two versions of product code.

## Ownership and conflicts

- Keep the actual last-published snapshot and its manifest outside the source tree.
  Do not substitute the current remote tree for that baseline after someone edits it.
- Export each new candidate to a new directory with `public_release.py export`.
  Export refuses existing destinations; `verify` checks hashes and source hygiene.
- `public_sync.py` plans additions, modifications, removals, executable-mode changes
  and internal symlinks. Removed source-managed files are explicitly deleted.
- `.github/`, `CI.md` and `RELEASE_PLAN.md` are repository-owned by default. They
  survive source updates. Additional `--protect PATH` options are literal path
  prefixes, not globs. New source changes colliding with protected content stop.
- Independent changes to source-managed files, including README, stop for review.
  A new source path colliding with an unowned remote path also stops. Nothing is
  silently merged or overwritten. Subsequent manifests record `sync_managed_paths`.

## Plan, review, prepare, publish

Use a clean checkout of the public repository at the current remote branch HEAD.
All output paths below should be outside that checkout and the candidate snapshot.

```sh
python3 automation/polar_station/public_sync.py plan \
  --repository /absolute/path/to/public-checkout \
  --previous /absolute/path/to/last-published-snapshot \
  --snapshot /absolute/path/to/new-verified-snapshot \
  --branch main --output /absolute/path/to/plan.json

# Review plan.json, including every deletion and the exact destination.
python3 automation/polar_station/public_sync.py apply \
  --plan /absolute/path/to/plan.json \
  --bundle /absolute/path/to/prepared.bundle \
  --receipt /absolute/path/to/receipt.json \
  --message 'Update reviewed Polar Station source snapshot'

# This separate flag authorizes the exact public upload described by the receipt.
python3 automation/polar_station/public_sync.py publish \
  --receipt /absolute/path/to/receipt.json --confirm-publication
```

`apply` rechecks the snapshot, baseline, clean worktree and remote HEAD, then
creates a commit and bundle using an isolated temporary Git index. It does not
change the caller's checkout or branch. The receipt includes the commit, tree,
parent, remote URL, branch, plan digest and bundle hash.

`publish` verifies the bundle and that its commit is a direct child of the expected
remote HEAD, checks the remote again, and uses an ordinary non-force push. It
never runs repository hooks, checkout filters or submodules. Comparison and push
are not atomic CAS; concurrent non-fast-forward changes are rejected by Git.
If the branch or snapshot changed, replan rather than force the update.

Publishing accepts HTTPS or git-user SSH URLs on github.com. SSH uses the existing
agent, strict host-key checking, batch mode, and no caller/repository SSH command overrides.
Publishing needs an existing non-interactive authentication environment.
The tool disables global/system Git configuration, does not set up credentials,
and accepts no token argument. Authentication failure is a blocker, not success.
This workflow is implemented and tested with local Git remotes; it does not
install a schedule or itself grant permission for future unattended publication.
After a confirmed publish, retain the exact published snapshot as the next
baseline, including preserved repository-owned files and its resulting manifest.

When a source-managed path was independently fixed after the recorded snapshot,
review that precise diff and integrate the required behavior into PolarInfra
first. `plan --reviewed-remote-changes identities.json` may then accept only the
explicit path -> {mode, sha256} identities reviewed. The original previous
snapshot is retained unchanged; the acceptance is bound into the plan and
rechecked during apply. This is not a blanket remote-conflict override.
