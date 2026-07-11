# TurboLog CLI Reference

> Terminal-first, local-first developer guide.

## Persona

TurboLog targets developers who:

- Work primarily in the terminal (tmux, shell, Neovim)
- Pipe logs from `tail`, `docker logs`, `journalctl`, or dev servers
- Want offline anomaly detection without SaaS or API keys
- Optionally use a local LLM (Ollama / LM Studio) for explanations

## Commands

| Command | Purpose |
|---------|---------|
| `watch` | Stream stdin, highlight anomalies in real time |
| `scan` | Read stdin to EOF, print summary report |
| `history` | Query local SQLite anomaly memory |
| `diagnose` | Summarize recurring patterns over a time window |
| `ui` | TUI dashboard (`--standalone` needs no server) |
| `completions` | Generate shell tab-completion scripts |
| `serve` | HTTP server (advanced; requires `--features server`) |

Running `turbolog` without a subcommand prints help.

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Success; no anomalies (`watch`/`scan`/`diagnose`) |
| `1` | Anomalies detected |
| `2` | Error (embedding failure, invalid args, I/O) |

Use in shell scripts:

```bash
turbolog scan < build.log || echo "investigate build.log"
```

## Flags

### `watch`

- `--only-anomalies` — hide normal and `[calibrating]` lines
- `--quiet` / `-q` — suppress stderr status (calibration, LLM connect)
- `--threshold <f32>` — override auto-calibrated score floor
- `--explain` — LLM explanation per anomaly
- `--llm-url`, `--llm-model` — LLM overrides

### `scan`

- `--format text|json`
- `--quiet` / `-q`
- `--threshold`, `--explain`, `--llm-url`, `--llm-model`

### `history`

- `--since 7d|24h|1h|30m`
- `--template <substring>` — filter lines
- `--top` — group by template, frequency order
- `--format text|json`
- `--limit <N>`

### `diagnose`

- `--since` (default `24h`)
- `--format text|json`
- `--explain` — LLM summary of top patterns
- `--limit <N>` (default `10`)
- `--quiet` / `-q`

## Shell Completions

```bash
# Bash
turbolog completions bash > ~/.local/share/bash-completion/completions/turbolog

# Zsh (add ~/.zfunc to fpath first)
turbolog completions zsh > ~/.zfunc/_turbolog

# Fish
turbolog completions fish > ~/.config/fish/completions/turbolog.fish
```

## Demo

Run the bundled demo (works from a source checkout, no install needed):

```bash
./scripts/demo.sh          # live streaming — only anomalies surface
./scripts/demo.sh explain  # same, plus local LLM explanations
```

It streams a realistic baseline so the detector calibrates, then injects genuine
anomalies (OOM, connection refused, segfault, kernel lockup) that get flagged live.

## Recipes

### Daily aliases

```bash
alias tw='turbolog watch --only-anomalies'
alias ts='turbolog scan --format json'
alias td='turbolog diagnose --since 24h'
```

### Dev workflow

```bash
cargo run 2>&1 | turbolog watch --explain
npm run dev 2>&1 | turbolog watch --only-anomalies --quiet
```

### History analysis

```bash
turbolog history --top --since 7d
turbolog diagnose --since 2h --explain
turbolog history --format json | jq '.[].template'
```

## Data Location

Anomaly history: `~/.local/share/turbolog/history.db` (or `$XDG_DATA_HOME/turbolog/history.db`).

## Architecture (CLI path)

```
stdin → LocalPipeline → Drain → VectorCache → AnomalyDetector
                              ↓
                    HistoryStore (SQLite, optional)
                              ↓
                    LlmClient (optional, --explain)
```

The CLI path does **not** require `serve`, WAL, or HTTP.
