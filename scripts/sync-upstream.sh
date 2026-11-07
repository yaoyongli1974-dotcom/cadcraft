#!/usr/bin/env bash
# Rebase the AICAD derivative onto upstream CADCraft, then prove it still works.
#
#   scripts/sync-upstream.sh                 # fetch upstream/main, merge on a sync branch
#   scripts/sync-upstream.sh --skip-gates    # merge only
#   scripts/sync-upstream.sh --remote upstream --branch main
#
# The script never pushes and never touches `main`: it leaves a `sync/upstream-*`
# branch checked out so conflicts can be resolved and the result reviewed. The
# last printed block says how to finish.
set -euo pipefail

remote=upstream
branch=main
run_gates=1

while [ $# -gt 0 ]; do
    case "$1" in
        --remote) remote=${2:?--remote needs a name}; shift 2 ;;
        --branch) branch=${2:?--branch needs a name}; shift 2 ;;
        --skip-gates) run_gates=0; shift ;;
        -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

say() { printf '\n== %s\n' "$*"; }

repo=$(git rev-parse --show-toplevel)
cd "$repo"

if ! git diff --quiet || ! git diff --cached --quiet || [ -n "$(git ls-files --others --exclude-standard)" ]; then
    echo "The working tree is not clean. Commit or stash first: the merge must be reviewable." >&2
    git status --short >&2
    exit 2
fi

say "Fetching $remote/$branch"
git fetch "$remote" "$branch"

# A shallow clone (this fork is one) can lack the merge base. Deepen until git
# can name it, so the merge does not silently become an unrelated-history merge.
for _ in 1 2 3; do
    if git merge-base HEAD "$remote/$branch" >/dev/null 2>&1; then break; fi
    say "No merge base yet; deepening the history"
    git fetch --deepen=200 "$remote" "$branch"
done
base=$(git merge-base HEAD "$remote/$branch") || {
    echo "Still no merge base with $remote/$branch. Fetch more history:" >&2
    echo "  git fetch --unshallow $remote" >&2
    exit 3
}
say "Merge base ${base:0:10}; upstream tip $(git rev-parse --short "$remote/$branch")"

stamp=$(date +%Y%m%d-%H%M)
tip=$(git rev-parse --short "$remote/$branch")
sync_branch="sync/upstream-$stamp-$tip"
git switch -c "$sync_branch"

say "Merging $remote/$branch into $sync_branch"
if ! git merge --no-ff --no-commit "$remote/$branch"; then
    echo >&2
    echo "Conflicts (resolve them, then git add and git commit):" >&2
    git diff --name-only --diff-filter=U >&2
    echo >&2
    echo "Hot spots in this fork: Cargo.toml, Cargo.lock, xtask/src/layers.rs," >&2
    echo "crates/ui-egui/src/{lib,docking,menus,icons,theme}.rs, i18n/zh.tsv," >&2
    echo "apps/cadcraft/src/main.rs. See docs/upstream-sync.md." >&2
    exit 4
fi

if git diff --cached --quiet; then
    say "Already up to date with $remote/$branch"
    git merge --abort 2>/dev/null || true
    git switch - >/dev/null
    git branch -D "$sync_branch" >/dev/null
    exit 0
fi

git commit --no-edit

if [ "$run_gates" -eq 1 ]; then
    say "cargo fmt --check"
    cargo fmt --all -- --check

    say "clippy (cadcraft-ai, cadcraft-ui-egui)"
    cargo clippy -p cadcraft-ai -p cadcraft-ui-egui --all-targets -- -D warnings

    say "tests (cadcraft-ai)"
    cargo test -p cadcraft-ai

    say "tests (cadcraft-ui-egui --lib)"
    cargo test -p cadcraft-ui-egui --lib

    say "layers"
    cargo xtask layers

    say "wasm check (cadcraft-ai)"
    cargo check -p cadcraft-ai --target wasm32-unknown-unknown
fi

say "Merged onto $sync_branch and verified"
cat <<EOF

Next:
  1. Look at the drawing once: cargo run -p cadcraft -- --sample
     Window > AI Chat, ask "以 0,0 为圆心画半径 30 的圆", apply, then Ctrl+Z.
  2. Merge and publish (the tag is your call; v0.0.N in this fork):
       git switch aicad-ai-assistant
       git merge --ff-only $sync_branch
       git push origin aicad-ai-assistant:main
  3. Optional: refresh the download binaries and cut a release.
EOF
