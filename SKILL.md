---
name: diari
description: Read and search the user's diari journal (Markdown notes synced to a git repo by the diari app). Use it when they ask what they did, wrote or have pending on a day or over a period.
---

# Diari

The journal is a git repo set in `$DIARI_REPO` (e.g. `git@github.com:you/notes.git`).
Keep a read-only copy at `$DIARI_PATH` (default `~/diari`). Before reading, run:

    D="${DIARI_PATH:-$HOME/diari}"
    [ -d "$D/.git" ] || git clone -q "$DIARI_REPO" "$D"
    git -C "$D" pull -q

If `$DIARI_REPO` is unset, ask the user for it. If git fails with a permission error, tell them this machine's SSH key needs read access to the repo.

- Daily notes are `YYYY_MM_DD.md` (e.g. `2026_11_27.md`). No file means nothing was written that day.
- `- HH:MM text` lines are things jotted, shared or dictated at that time.
- `## Calendar` lists that day's events; `## To do` lists calendar tasks. `- [ ]` is pending, `- [x]` done.
- Attachments are in `<note>.md.files/`.
- Search with `grep -rni --include='*.md' --exclude=CONFIG.md "<text>" "$D"`.

Rules: read only, never edit, commit or push. Never show `CONFIG.md` (it may hold private calendar links). Say which day each thing comes from. Answer in the user's language.
