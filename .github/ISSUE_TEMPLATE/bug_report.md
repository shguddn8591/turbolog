---
name: Bug Report
about: Report a problem with TurboLog
title: "[BUG] "
labels: bug
assignees: ""
---

**Describe the bug**
A clear and concise description of what went wrong.

**To Reproduce**
Prefer a **CLI** reproduction:

```bash
# example
printf 'line1\nline2\n' | turbolog scan
# or
tail -f /path/to.log | turbolog watch --threshold 0.5
```

Steps:
1. …
2. …
3. …

If this is about experimental `serve` / `ui`, say so and include the feature build (`--features server` / `tui`).

**Expected behavior**
What you expected instead.

**Logs / output**
Paste stderr/stdout (redact secrets).

**Environment**
- OS: [e.g. Ubuntu 22.04, macOS]
- Install method: [cargo install / release binary / from source]
- TurboLog version: [`turbolog --help` / crates.io version]
- Features: [default / server / tui]
- Local LLM (if `--explain`): [none / Ollama / LM Studio / URL]

**Additional context**
Model dir overrides, unusual log formats, etc.
