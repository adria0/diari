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

- Daily notes are `diari/YYYY_MM_DD.md` (e.g. `diari/2026_11_27.md`; older repos keep them at the top). No file means nothing was written that day.
- Other top-level folders with their own `CONFIG.md` are workspaces of notes on a subject (`feina/`, `familia/`…).
- `- HH:MM text` lines are things jotted, shared or dictated at that time.
- `## Calendar` lists that day's events; `## To do` lists calendar tasks. `- [ ]` is pending, `- [x]` done.
- Attachments are in `<note>.md.files/`.
- `[[Name]]` or `[[Name|text]]` links to the note `Name.md` (by file name, or path for subfolders). To find what mentions a note, grep for `[[Name`.
- Search with `grep -rni --include='*.md' --exclude=CONFIG.md "<text>" "$D"`.

Rules: read only, never edit, commit or push. Never show `CONFIG.md` (it may hold private calendar links). Say which day each thing comes from. Answer in the user's language.
